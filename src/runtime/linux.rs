// Copyright 2026 Victor Stewart
// SPDX-License-Identifier: Apache-2.0

use super::{
    build_cell_request_path, write_build_cell_request, BackendCaps, BuildCellResult, BuildCellSpec,
    CacheSpec, CleanupPolicy, ExportSpec, ImportSpec, NetworkPolicy, ResourceLimits, WorkspaceSeed,
};
use anyhow::{bail, Context, Result};
use flate2::read::GzDecoder;
use sha2::{Digest, Sha256};
use std::ffi::{CString, OsStr};
use std::fs;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::os::unix::process::ExitStatusExt;
use std::path::Component;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use tar::Archive;

pub const RUN_HELPER_DIR: &str = "/.metalor-run";
const ISOLATION_ENV: &str = "METALOR_PRIVATE_NS";
const RUNTIME_PREFIX_ENV: &str = "METALOR_RUNTIME_ROOT_PREFIX";
const ROOT_SENTINEL: &str = ".metalor-root";
const BUILD_CELL_WORKSPACE_DIR: &str = "workspace";
const BUILD_CELL_IMPORTS_DIR: &str = "imports";
const BUILD_CELL_CACHES_DIR: &str = "caches";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BindMount {
    pub source: PathBuf,
    pub destination: String,
    pub read_only: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContainerRunCommand {
    pub root: PathBuf,
    pub cwd: String,
    pub mounts: Vec<BindMount>,
    pub env: Vec<(String, String)>,
    pub emulator: Option<String>,
    pub executable: String,
    pub argv: Vec<String>,
}

#[derive(Clone, Debug)]
struct PortableMount {
    source: PathBuf,
    destination: String,
}

pub fn helper_binary_path(binary_name: &str) -> String {
    format!("{RUN_HELPER_DIR}/{binary_name}")
}

pub fn backend_caps() -> BackendCaps {
    BackendCaps {
        oci_rootfs: true,
        live_bind_mounts: true,
        foreign_arch_exec: true,
        per_job_network_toggle: true,
        profile_selected_network: false,
    }
}

pub fn build_cell_reexec_command(
    current_exe: &Path,
    subcommand: &str,
    runtime_root_prefix: &Path,
    spec: &BuildCellSpec,
) -> Result<Command> {
    validate_build_cell_spec(spec)?;
    let canonical_prefix = canonicalize_runtime_root_prefix(runtime_root_prefix)?;
    let canonical_root = prepare_runtime_root(spec.root.as_path(), &canonical_prefix)?;
    let canonical_scratch =
        prepare_runtime_workspace(spec.scratch.as_path(), &canonical_prefix, "build scratch")?;
    validate_distinct_runtime_paths(&canonical_root, &canonical_scratch)?;

    reset_build_cell_stage(&canonical_scratch)?;
    stage_build_cell(spec, &canonical_scratch)?;

    let request_path = build_cell_request_path(&canonical_scratch);
    write_build_cell_request(&request_path, spec)?;

    build_isolation_reexec_command(
        current_exe,
        subcommand,
        &canonical_prefix,
        &[
            String::from("--build-cell-request"),
            request_path.display().to_string(),
        ],
        matches!(spec.network, NetworkPolicy::Disabled),
    )
}

pub fn run_build_cell(spec: &BuildCellSpec) -> Result<()> {
    validate_build_cell_spec(spec)?;
    let command = build_cell_container_command(spec)?;
    ensure_isolated_runtime(&command)?;
    run_container_command_inner_with_options(
        &command,
        PortableRunOptions {
            mount_host_network_config: matches!(spec.network, NetworkPolicy::Enabled),
            limits: spec.limits,
        },
    )
}

pub fn finalize_build_cell(
    spec: &BuildCellSpec,
    command_succeeded: bool,
) -> Result<BuildCellResult> {
    validate_build_cell_spec(spec)?;
    let stage = build_cell_stage(spec);
    sync_build_cell_caches(spec, &stage)?;
    sync_build_cell_exports(spec, &stage, command_succeeded)?;

    let scratch_preserved = match spec.cleanup {
        CleanupPolicy::Always => false,
        CleanupPolicy::PreserveOnFailure => !command_succeeded,
        CleanupPolicy::Never => true,
    };
    if !scratch_preserved && stage.root.exists() {
        fs::remove_dir_all(&stage.root)
            .with_context(|| format!("failed to remove build scratch {}", stage.root.display()))?;
    }
    Ok(BuildCellResult { scratch_preserved })
}

pub fn build_unshare_reexec_command(
    current_exe: &Path,
    subcommand: &str,
    runtime_root_prefix: &Path,
    command: &ContainerRunCommand,
) -> Result<Command> {
    validate_container_request(command)?;
    let canonical_prefix = canonicalize_runtime_root_prefix(runtime_root_prefix)?;
    let canonical_root = prepare_runtime_root(&command.root, &canonical_prefix)?;
    let mut trailing_args = vec![
        String::from("--root"),
        canonical_root.display().to_string(),
        String::from("--cwd"),
        command.cwd.clone(),
    ];
    for mount in &command.mounts {
        trailing_args.push(String::from("--mount-source"));
        trailing_args.push(mount.source.display().to_string());
        trailing_args.push(String::from("--mount-dest"));
        trailing_args.push(mount.destination.clone());
        trailing_args.push(String::from("--mount-mode"));
        trailing_args.push(if mount.read_only {
            String::from("ro")
        } else {
            String::from("rw")
        });
    }
    for (key, value) in &command.env {
        trailing_args.push(String::from("--env"));
        trailing_args.push(format!("{key}={value}"));
    }
    if let Some(emulator) = &command.emulator {
        trailing_args.push(String::from("--emulator"));
        trailing_args.push(emulator.clone());
    }
    trailing_args.push(String::from("--executable"));
    trailing_args.push(command.executable.clone());
    trailing_args.extend(command.argv.iter().cloned());

    build_isolation_reexec_command(
        current_exe,
        subcommand,
        &canonical_prefix,
        &trailing_args,
        false,
    )
}

pub fn run_isolated_container_command(command: &ContainerRunCommand) -> Result<()> {
    validate_container_request(command)?;
    ensure_isolated_runtime(command)?;
    run_container_command_inner(command)
}

pub fn prepare_runtime_emulator(
    runtime_root: &Path,
    host_arch: &str,
    guest_arch: &str,
) -> Result<Option<String>> {
    let normalized_host = normalize_arch_name(host_arch)?;
    let normalized_guest = normalize_arch_name(guest_arch)?;
    if normalized_host == normalized_guest {
        return Ok(None);
    }

    if runtime_root == Path::new("/") {
        bail!("runtime root for emulator staging must not be /");
    }
    let runtime_root = prepare_host_directory(runtime_root, "runtime root")?;

    let required_qemu = required_qemu_binary(normalized_guest);
    let host_qemu_path = find_tool_in_path(required_qemu).with_context(|| {
        format!(
            "foreign-architecture execution for guest {} on host {} requires {} in PATH",
            normalized_guest, normalized_host, required_qemu
        )
    })?;
    let resolved_host_qemu = host_qemu_path
        .canonicalize()
        .unwrap_or_else(|_| host_qemu_path.clone());
    let emulator_path = helper_binary_path(required_qemu);
    let host_destination = prepare_container_mount_target(
        &runtime_root,
        &emulator_path,
        false,
        "runtime helper path",
    )?;
    fs::copy(&resolved_host_qemu, &host_destination).with_context(|| {
        format!(
            "failed to install {} into runtime root at {}",
            required_qemu,
            host_destination.display()
        )
    })?;
    let mode = fs::metadata(&resolved_host_qemu)?.permissions().mode();
    fs::set_permissions(&host_destination, fs::Permissions::from_mode(mode))?;
    Ok(Some(emulator_path))
}

pub fn prepare_oci_rootfs(
    reference: &str,
    runtime_root_prefix: &Path,
    runtime_package_root: &Path,
    cache_root: Option<&Path>,
    requested_arch: Option<&str>,
) -> Result<PathBuf> {
    let canonical_prefix = canonicalize_runtime_root_prefix(runtime_root_prefix)?;
    let package_root =
        prepare_runtime_workspace(runtime_package_root, &canonical_prefix, "OCI package root")?;
    let oci_root = package_root.join("oci");
    let bundle_root = oci_root.join("bundle");
    if bundle_root.exists() {
        fs::remove_dir_all(&bundle_root)
            .with_context(|| format!("failed to remove {}", bundle_root.display()))?;
    }
    fs::create_dir_all(&oci_root)
        .with_context(|| format!("failed to create {}", oci_root.display()))?;

    let source_reference = normalize_oci_reference(reference);
    let requested_oci_arch = requested_oci_arch(requested_arch)?;
    let layout_root = match cache_root {
        Some(cache_root) => {
            prepare_cached_oci_layout(&source_reference, cache_root, requested_oci_arch)?
        }
        None => {
            let layout_root = oci_root.join("layout");
            if layout_root.exists() {
                fs::remove_dir_all(&layout_root)
                    .with_context(|| format!("failed to remove {}", layout_root.display()))?;
            }
            populate_oci_layout(&source_reference, &layout_root, requested_oci_arch)?;
            layout_root
        }
    };
    run_command(
        Command::new("umoci")
            .arg("unpack")
            .arg("--rootless")
            .arg("--keep-dirlinks")
            .arg("--image")
            .arg(format!("{}:image", layout_root.display()))
            .arg(&bundle_root),
        &format!(
            "unpack OCI image {} into {}",
            layout_root.display(),
            bundle_root.display()
        ),
    )?;

    let rootfs = bundle_root.join("rootfs");
    if !rootfs.is_dir() {
        bail!(
            "OCI image '{}' did not unpack to a rootfs at {}",
            reference,
            rootfs.display()
        );
    }
    Ok(rootfs)
}

fn prepare_cached_oci_layout(
    reference: &str,
    cache_root: &Path,
    requested_oci_arch: Option<&'static str>,
) -> Result<PathBuf> {
    let cache_root = prepare_host_directory(cache_root, "OCI cache root")?;
    let cache_entry_root = cache_root.join(cache_key(reference, requested_oci_arch));
    let layout_root = cache_entry_root.join("layout");
    let reference_file = cache_entry_root.join("reference.txt");
    let cache_identity = cache_identity(reference, requested_oci_arch);
    let cache_reusable = layout_root.join("index.json").exists()
        && fs::read_to_string(&reference_file)
            .map(|value| value == cache_identity)
            .unwrap_or(false);
    if !cache_reusable {
        if cache_entry_root.exists() {
            fs::remove_dir_all(&cache_entry_root)
                .with_context(|| format!("failed to remove {}", cache_entry_root.display()))?;
        }
        fs::create_dir_all(&cache_entry_root)
            .with_context(|| format!("failed to create {}", cache_entry_root.display()))?;
        populate_oci_layout(reference, &layout_root, requested_oci_arch)?;
        fs::write(&reference_file, cache_identity)
            .with_context(|| format!("failed to write {}", reference_file.display()))?;
    }
    Ok(layout_root)
}

fn populate_oci_layout(
    reference: &str,
    layout_root: &Path,
    requested_oci_arch: Option<&'static str>,
) -> Result<()> {
    let destination_reference = format!("oci:{}:image", layout_root.display());
    let mut command = Command::new("skopeo");
    command
        .arg("copy")
        .arg("--quiet")
        .arg("--multi-arch")
        .arg("system");
    if let Some(requested_oci_arch) = requested_oci_arch {
        command.arg("--override-arch").arg(requested_oci_arch);
    }
    command.arg(reference).arg(&destination_reference);
    run_command(
        &mut command,
        &format!("copy OCI image {reference} into {}", layout_root.display()),
    )
}

fn cache_key(reference: &str, requested_oci_arch: Option<&'static str>) -> String {
    format!(
        "{:x}",
        Sha256::digest(cache_identity(reference, requested_oci_arch).as_bytes())
    )
}

fn cache_identity(reference: &str, requested_oci_arch: Option<&'static str>) -> String {
    format!(
        "{reference}\narch={}",
        requested_oci_arch.unwrap_or("system")
    )
}

fn run_container_command_inner(command: &ContainerRunCommand) -> Result<()> {
    run_container_command_inner_with_options(
        command,
        PortableRunOptions {
            mount_host_network_config: true,
            limits: ResourceLimits::default(),
        },
    )
}

#[derive(Clone, Copy, Debug)]
struct PortableRunOptions {
    mount_host_network_config: bool,
    limits: ResourceLimits,
}

fn run_container_command_inner_with_options(
    command: &ContainerRunCommand,
    options: PortableRunOptions,
) -> Result<()> {
    fs::create_dir_all(&command.root)?;
    apply_bind_mounts(command)?;

    let proc_dir = command.root.join("proc");
    fs::create_dir_all(&proc_dir)?;
    run_command(
        Command::new("mount")
            .args(["-t", "proc", "proc"])
            .arg(&proc_dir),
        "mount proc for container command",
    )?;
    if command.emulator.is_some() {
        let binfmt_dir = command.root.join("proc/sys/fs/binfmt_misc");
        fs::create_dir_all(&binfmt_dir)?;
        run_command(
            Command::new("mount")
                .args(["-t", "binfmt_misc", "binfmt_misc"])
                .arg(&binfmt_dir),
            "mount binfmt_misc for container command",
        )?;
    }
    if options.mount_host_network_config && !has_mount_covering(command, "/etc/resolv.conf") {
        mount_host_resolv_conf(&command.root)?;
    }
    mount_host_run_devices(command)?;

    chroot_into(&command.root)?;
    let private_binfmt_rule = if let Some(emulator) = &command.emulator {
        Some(register_private_binfmt(emulator)?)
    } else {
        None
    };

    let result = run_container_process(command, options.limits);
    let unregister_result = if let Some(rule_name) = &private_binfmt_rule {
        unregister_private_binfmt(rule_name)
    } else {
        Ok(())
    };

    match (result, unregister_result) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), Ok(())) => Err(error),
        (Ok(()), Err(error)) => Err(error),
        (Err(error), Err(_)) => Err(error),
    }
}

fn ensure_isolated_runtime(command: &ContainerRunCommand) -> Result<()> {
    if std::env::var_os(ISOLATION_ENV).is_none() {
        bail!("refusing to run container command without {ISOLATION_ENV}=1");
    }
    ensure_private_mount_namespace()?;

    let canonical_prefix = canonicalize_runtime_root_prefix(Path::new(
        &std::env::var(RUNTIME_PREFIX_ENV)
            .with_context(|| format!("missing required env {RUNTIME_PREFIX_ENV}"))?,
    ))?;
    let canonical_root = canonicalize_runtime_root(&command.root)?;
    if !canonical_root.starts_with(&canonical_prefix) {
        bail!(
            "refusing to run container command outside runtime root prefix: {} is not under {}",
            canonical_root.display(),
            canonical_prefix.display()
        );
    }
    ensure_runtime_root_sentinel(&canonical_root, &canonical_prefix)?;
    Ok(())
}

fn build_isolation_reexec_command(
    current_exe: &Path,
    subcommand: &str,
    canonical_prefix: &Path,
    trailing_args: &[String],
    disable_network: bool,
) -> Result<Command> {
    let mut process = Command::new("unshare");
    process.args(["--fork", "--pid", "--mount", "--uts", "--ipc"]);
    if disable_network {
        process.arg("--net");
    }
    process
        .arg("--")
        .arg(current_exe)
        .arg(subcommand)
        .args(trailing_args);
    process
        .env(ISOLATION_ENV, "1")
        .env(RUNTIME_PREFIX_ENV, canonical_prefix.as_os_str());
    Ok(process)
}

#[derive(Clone, Debug)]
struct BuildCellStage {
    root: PathBuf,
    workspace: PathBuf,
    imports_root: PathBuf,
    caches_root: PathBuf,
}

impl BuildCellStage {
    fn import_path(&self, index: usize) -> PathBuf {
        self.imports_root.join(index.to_string())
    }

    fn cache_path(&self, index: usize) -> PathBuf {
        self.caches_root.join(index.to_string())
    }
}

fn build_cell_stage(spec: &BuildCellSpec) -> BuildCellStage {
    BuildCellStage {
        root: spec.scratch.as_path().to_path_buf(),
        workspace: spec.scratch.as_path().join(BUILD_CELL_WORKSPACE_DIR),
        imports_root: spec.scratch.as_path().join(BUILD_CELL_IMPORTS_DIR),
        caches_root: spec.scratch.as_path().join(BUILD_CELL_CACHES_DIR),
    }
}

fn validate_build_cell_spec(spec: &BuildCellSpec) -> Result<()> {
    if spec.root.as_path() == Path::new("/") {
        bail!("build cell root must not be /");
    }
    if spec.scratch.as_path() == Path::new("/") {
        bail!("build scratch must not be /");
    }

    validate_host_absolute_path(spec.root.as_path(), "build cell root")?;
    validate_host_absolute_path(spec.scratch.as_path(), "build scratch")?;
    validate_container_absolute_path(spec.workspace_path.as_str(), "build cell workspace path")?;
    if spec.workspace_path.as_str() == "/" {
        bail!("build cell workspace path must not be /");
    }

    match &spec.workspace_seed {
        WorkspaceSeed::Empty => {}
        WorkspaceSeed::SnapshotDir(path) => {
            validate_host_absolute_path(path.as_path(), "build workspace seed")?;
        }
        WorkspaceSeed::Archive(path) => {
            validate_host_absolute_path(path.as_path(), "build workspace archive")?;
        }
    }

    for import in &spec.imports {
        validate_import_spec(import)?;
    }
    for cache in &spec.caches {
        validate_cache_spec(cache)?;
    }
    for export in &spec.exports {
        validate_export_spec(export)?;
    }
    validate_portable_mount_destinations(spec)?;
    validate_container_absolute_path(spec.command.cwd.as_str(), "build cell cwd")?;
    validate_container_executable(&spec.command.executable)?;
    validate_container_environment(&spec.env)?;
    Ok(())
}

fn validate_import_spec(import: &ImportSpec) -> Result<()> {
    validate_host_absolute_path(import.source.as_path(), "build import source")?;
    validate_container_absolute_path(import.destination.as_str(), "build import destination")?;
    if import.destination.as_str() == "/" {
        bail!("build import destination must not be /");
    }
    Ok(())
}

fn validate_cache_spec(cache: &CacheSpec) -> Result<()> {
    validate_host_absolute_path(cache.source.as_path(), "build cache source")?;
    validate_container_absolute_path(cache.destination.as_str(), "build cache destination")?;
    if cache.destination.as_str() == "/" {
        bail!("build cache destination must not be /");
    }
    Ok(())
}

fn validate_export_spec(export: &ExportSpec) -> Result<()> {
    validate_container_absolute_path(export.source.as_str(), "build export source")?;
    validate_host_absolute_path(export.destination.as_path(), "build export destination")?;
    Ok(())
}

fn validate_portable_mount_destinations(spec: &BuildCellSpec) -> Result<()> {
    let mut destinations = Vec::with_capacity(spec.imports.len() + spec.caches.len());
    for import in &spec.imports {
        destinations.push(("import", import.destination.as_str()));
    }
    for cache in &spec.caches {
        destinations.push(("cache", cache.destination.as_str()));
    }
    for index in 0..destinations.len() {
        for other in (index + 1)..destinations.len() {
            let (left_kind, left) = destinations[index];
            let (right_kind, right) = destinations[other];
            if mount_covers(left, right) || mount_covers(right, left) {
                bail!(
                    "portable {} destination {} conflicts with {} destination {}",
                    left_kind,
                    left,
                    right_kind,
                    right
                );
            }
        }
    }
    Ok(())
}

fn validate_distinct_runtime_paths(root: &Path, scratch: &Path) -> Result<()> {
    if root == scratch {
        bail!("build scratch must not equal the build cell root");
    }
    if root.starts_with(scratch) || scratch.starts_with(root) {
        bail!(
            "build scratch {} must not overlap build cell root {}",
            scratch.display(),
            root.display()
        );
    }
    Ok(())
}

fn reset_build_cell_stage(scratch: &Path) -> Result<()> {
    if scratch.exists() {
        fs::remove_dir_all(scratch)
            .with_context(|| format!("failed to reset build scratch {}", scratch.display()))?;
    }
    fs::create_dir_all(scratch)
        .with_context(|| format!("failed to create build scratch {}", scratch.display()))?;
    Ok(())
}

fn stage_build_cell(spec: &BuildCellSpec, _scratch: &Path) -> Result<()> {
    let stage = build_cell_stage(spec);
    fs::create_dir_all(&stage.workspace)
        .with_context(|| format!("failed to create {}", stage.workspace.display()))?;
    fs::create_dir_all(&stage.imports_root)
        .with_context(|| format!("failed to create {}", stage.imports_root.display()))?;
    fs::create_dir_all(&stage.caches_root)
        .with_context(|| format!("failed to create {}", stage.caches_root.display()))?;

    match &spec.workspace_seed {
        WorkspaceSeed::Empty => {}
        WorkspaceSeed::SnapshotDir(source) => {
            copy_directory_contents(source.as_path(), &stage.workspace)?
        }
        WorkspaceSeed::Archive(archive) => {
            unpack_workspace_archive(archive.as_path(), &stage.workspace)?
        }
    }

    for (index, import) in spec.imports.iter().enumerate() {
        copy_path(import.source.as_path(), &stage.import_path(index))?;
    }
    for (index, cache) in spec.caches.iter().enumerate() {
        let destination = stage.cache_path(index);
        if cache.source.as_path().exists() {
            copy_path(cache.source.as_path(), &destination)?;
        } else {
            fs::create_dir_all(&destination)
                .with_context(|| format!("failed to create {}", destination.display()))?;
        }
    }
    Ok(())
}

fn build_cell_container_command(spec: &BuildCellSpec) -> Result<ContainerRunCommand> {
    let stage = build_cell_stage(spec);
    let mut mounts = Vec::with_capacity(1 + spec.imports.len() + spec.caches.len());
    mounts.push(BindMount {
        source: stage.workspace.clone(),
        destination: spec.workspace_path.as_str().to_string(),
        read_only: false,
    });
    for (index, import) in spec.imports.iter().enumerate() {
        mounts.push(BindMount {
            source: stage.import_path(index),
            destination: import.destination.as_str().to_string(),
            read_only: true,
        });
    }
    for (index, cache) in spec.caches.iter().enumerate() {
        mounts.push(BindMount {
            source: stage.cache_path(index),
            destination: cache.destination.as_str().to_string(),
            read_only: false,
        });
    }

    Ok(ContainerRunCommand {
        root: spec.root.as_path().to_path_buf(),
        cwd: spec.command.cwd.as_str().to_string(),
        mounts,
        env: spec.env.clone(),
        emulator: None,
        executable: spec.command.executable.clone(),
        argv: spec.command.argv.clone(),
    })
}

fn sync_build_cell_caches(spec: &BuildCellSpec, stage: &BuildCellStage) -> Result<()> {
    for (index, cache) in spec.caches.iter().enumerate() {
        copy_path(&stage.cache_path(index), cache.source.as_path())?;
    }
    Ok(())
}

fn sync_build_cell_exports(
    spec: &BuildCellSpec,
    stage: &BuildCellStage,
    command_succeeded: bool,
) -> Result<()> {
    for export in &spec.exports {
        let source = resolve_export_source(spec, stage, export.source.as_str())?;
        if !source.exists() {
            if command_succeeded {
                bail!(
                    "expected build export {} to exist at {}",
                    export.source.as_str(),
                    source.display()
                );
            }
            continue;
        }
        copy_path(&source, export.destination.as_path())?;
    }
    Ok(())
}

fn resolve_export_source(
    spec: &BuildCellSpec,
    stage: &BuildCellStage,
    target: &str,
) -> Result<PathBuf> {
    let mut best: Option<PortableMount> = Some(PortableMount {
        source: stage.workspace.clone(),
        destination: spec.workspace_path.as_str().to_string(),
    });
    for (index, import) in spec.imports.iter().enumerate() {
        let candidate = PortableMount {
            source: stage.import_path(index),
            destination: import.destination.as_str().to_string(),
        };
        best = choose_better_mount(best, candidate, target);
    }
    for (index, cache) in spec.caches.iter().enumerate() {
        let candidate = PortableMount {
            source: stage.cache_path(index),
            destination: cache.destination.as_str().to_string(),
        };
        best = choose_better_mount(best, candidate, target);
    }

    if let Some(best) = best.filter(|mount| mount_covers(&mount.destination, target)) {
        return map_mount_source(&best.source, &best.destination, target);
    }
    Ok(spec.root.as_path().join(target.trim_start_matches('/')))
}

fn choose_better_mount(
    current: Option<PortableMount>,
    candidate: PortableMount,
    target: &str,
) -> Option<PortableMount> {
    if !mount_covers(&candidate.destination, target) {
        return current;
    }
    match current {
        Some(existing)
            if mount_covers(&existing.destination, target)
                && existing.destination.len() >= candidate.destination.len() =>
        {
            Some(existing)
        }
        _ => Some(candidate),
    }
}

fn map_mount_source(source: &Path, destination: &str, target: &str) -> Result<PathBuf> {
    if target == destination {
        return Ok(source.to_path_buf());
    }
    let relative = target
        .strip_prefix(destination)
        .and_then(|value| value.strip_prefix('/'))
        .with_context(|| format!("mount destination {} did not cover {}", destination, target))?;
    Ok(source.join(relative))
}

fn canonicalize_runtime_root_prefix(prefix: &Path) -> Result<PathBuf> {
    if prefix == Path::new("/") {
        bail!("runtime root prefix must not be /");
    }
    validate_host_absolute_path(prefix, "runtime root prefix")?;
    fs::canonicalize(prefix).with_context(|| {
        format!(
            "failed to canonicalize runtime root prefix {}",
            prefix.display()
        )
    })
}

fn prepare_runtime_root(root: &Path, canonical_prefix: &Path) -> Result<PathBuf> {
    let canonical_root = prepare_runtime_workspace(root, canonical_prefix, "container root")?;
    write_runtime_root_sentinel(&canonical_root, canonical_prefix)?;
    Ok(canonical_root)
}

fn prepare_runtime_workspace(
    root: &Path,
    canonical_prefix: &Path,
    context: &str,
) -> Result<PathBuf> {
    if root == Path::new("/") {
        bail!("container root must not be /");
    }
    validate_host_absolute_path(root, context)?;
    if !root.starts_with(canonical_prefix) {
        bail!(
            "{} {} must live under runtime root prefix {}",
            context,
            root.display(),
            canonical_prefix.display()
        );
    }
    let prepared_root = prepare_host_directory(root, context)?;
    let canonical_root = fs::canonicalize(&prepared_root).with_context(|| {
        format!(
            "failed to canonicalize {} {}",
            context,
            prepared_root.display()
        )
    })?;
    if !canonical_root.starts_with(canonical_prefix) {
        bail!(
            "{} {} must live under runtime root prefix {}",
            context,
            canonical_root.display(),
            canonical_prefix.display()
        );
    }
    Ok(canonical_root)
}

fn canonicalize_runtime_root(root: &Path) -> Result<PathBuf> {
    if root == Path::new("/") {
        bail!("container root must not be /");
    }
    validate_host_absolute_path(root, "container root")?;
    fs::canonicalize(root)
        .with_context(|| format!("failed to canonicalize container root {}", root.display()))
}

fn write_runtime_root_sentinel(root: &Path, canonical_prefix: &Path) -> Result<()> {
    let sentinel_path = prepare_runtime_root_sentinel_path(root)?;
    fs::write(
        &sentinel_path,
        canonical_prefix.as_os_str().as_encoded_bytes(),
    )
    .with_context(|| {
        format!(
            "failed to write runtime root sentinel {}",
            sentinel_path.display()
        )
    })?;
    Ok(())
}

fn ensure_runtime_root_sentinel(root: &Path, canonical_prefix: &Path) -> Result<()> {
    let sentinel_path = prepare_runtime_root_sentinel_path(root)?;
    let expected = canonical_prefix.as_os_str().as_encoded_bytes();
    let actual = fs::read(&sentinel_path)
        .with_context(|| format!("missing runtime root sentinel {}", sentinel_path.display()))?;
    if actual != expected {
        bail!(
            "runtime root sentinel {} did not match the declared runtime root prefix",
            sentinel_path.display()
        );
    }
    Ok(())
}

fn ensure_private_mount_namespace() -> Result<()> {
    let self_ns = fs::read_link("/proc/self/ns/mnt")
        .context("failed to read /proc/self/ns/mnt for container isolation check")?;
    let init_ns = fs::read_link("/proc/1/ns/mnt")
        .context("failed to read /proc/1/ns/mnt for container isolation check")?;
    if self_ns == init_ns {
        bail!("refusing to run container command in the host mount namespace");
    }
    Ok(())
}

fn run_container_process(command: &ContainerRunCommand, limits: ResourceLimits) -> Result<()> {
    chdir_into(&command.cwd)?;
    apply_resource_limits(limits)?;

    let mut process = Command::new(&command.executable);
    process
        .args(&command.argv)
        .env_clear()
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    for (key, value) in &command.env {
        process.env(key, value);
    }

    let status = process
        .status()
        .with_context(|| format!("failed to spawn container command {}", command.executable))?;
    if status.success() {
        return Ok(());
    }
    if let Some(signal) = status.signal() {
        bail!(
            "container command {} terminated by signal {}",
            command.executable,
            signal
        );
    }
    bail!(
        "container command {} exited with status {}",
        command.executable,
        status
    );
}

fn apply_bind_mounts(command: &ContainerRunCommand) -> Result<()> {
    for mount in &command.mounts {
        let metadata = fs::metadata(&mount.source)
            .with_context(|| format!("missing bind mount source {}", mount.source.display()))?;
        let target = prepare_container_mount_target(
            &command.root,
            &mount.destination,
            metadata.is_dir(),
            "container mount destination",
        )?;

        run_command(
            Command::new("mount")
                .args(["--bind"])
                .arg(&mount.source)
                .arg(&target),
            &format!(
                "bind mount {} to {}",
                mount.source.display(),
                target.display()
            ),
        )?;
        if mount.read_only {
            run_command(
                Command::new("mount")
                    .args(["-o", "remount,bind,ro"])
                    .arg(&target),
                &format!("remount {} read-only", target.display()),
            )?;
        }
    }
    Ok(())
}

fn validate_container_request(command: &ContainerRunCommand) -> Result<()> {
    validate_container_absolute_path(&command.cwd, "container cwd")?;
    validate_container_executable(&command.executable)?;
    validate_container_environment(&command.env)?;
    if let Some(emulator) = &command.emulator {
        validate_container_absolute_path(emulator, "container emulator")?;
        if !emulator.starts_with(&format!("{RUN_HELPER_DIR}/")) {
            bail!(
                "container emulator must live under {}: {}",
                RUN_HELPER_DIR,
                emulator
            );
        }
    }
    for mount in &command.mounts {
        validate_bind_mount_source(&mount.source)?;
        validate_container_destination(&mount.destination)?;
    }
    Ok(())
}

fn validate_bind_mount_source(source: &Path) -> Result<()> {
    validate_host_absolute_path(source, "bind mount source")
}

fn validate_container_executable(executable: &str) -> Result<()> {
    let path = Path::new(executable);
    if path.is_absolute() {
        return validate_container_absolute_path(executable, "container executable");
    }
    match path.components().next() {
        Some(Component::Normal(_)) if path.components().count() == 1 => Ok(()),
        Some(Component::ParentDir) => {
            bail!("container executable must not contain '..': {executable}")
        }
        _ => bail!("container executable must be absolute or a bare command name: {executable}"),
    }
}

fn validate_container_absolute_path(path: &str, context: &str) -> Result<()> {
    let path = Path::new(path);
    if !path.is_absolute() {
        bail!("{context} must be absolute: {}", path.display());
    }
    for component in path.components() {
        match component {
            Component::RootDir | Component::Normal(_) => {}
            Component::CurDir => {}
            Component::ParentDir => {
                bail!("{context} must not contain '..': {}", path.display())
            }
            Component::Prefix(_) => bail!("unsupported {context}: {}", path.display()),
        }
    }
    Ok(())
}

fn validate_container_destination(destination: &str) -> Result<()> {
    validate_container_absolute_path(destination, "container mount destination")
}

fn validate_container_environment(env: &[(String, String)]) -> Result<()> {
    for (key, value) in env {
        if key.is_empty() {
            bail!("container environment keys must not be empty");
        }
        if key.contains('=') {
            bail!("container environment keys must not contain '=': {key}");
        }
        if key.contains('\0') {
            bail!("container environment keys must not contain NUL bytes");
        }
        if value.contains('\0') {
            bail!("container environment values must not contain NUL bytes");
        }
    }
    Ok(())
}

fn has_mount_covering(command: &ContainerRunCommand, target: &str) -> bool {
    command
        .mounts
        .iter()
        .any(|mount| mount_covers(&mount.destination, target))
}

fn mount_covers(mount_destination: &str, target: &str) -> bool {
    if mount_destination == target || mount_destination == "/" {
        return true;
    }
    let Some(stripped) = target.strip_prefix(mount_destination) else {
        return false;
    };
    stripped.starts_with('/')
}

fn mount_host_resolv_conf(root: &Path) -> Result<()> {
    let host_resolv = Path::new("/etc/resolv.conf");
    if !host_resolv.exists() {
        return Ok(());
    }

    let target = prepare_container_mount_target(
        root,
        "/etc/resolv.conf",
        false,
        "container auto-mount target",
    )?;
    run_command(
        Command::new("mount")
            .args(["--bind"])
            .arg(host_resolv)
            .arg(&target),
        "bind /etc/resolv.conf for container command",
    )?;
    Ok(())
}

fn mount_host_run_devices(command: &ContainerRunCommand) -> Result<()> {
    for device in ["/dev/null", "/dev/zero", "/dev/random", "/dev/urandom"] {
        if has_mount_covering(command, device) {
            continue;
        }
        let host_device = Path::new(device);
        if !host_device.exists() {
            continue;
        }

        let target = prepare_container_mount_target(
            &command.root,
            device,
            false,
            "container auto-mount target",
        )?;
        run_command(
            Command::new("mount")
                .args(["--bind"])
                .arg(host_device)
                .arg(&target),
            &format!("bind {device} for container command"),
        )?;
    }
    Ok(())
}

fn register_private_binfmt(emulator_path: &str) -> Result<String> {
    let rule_name = private_binfmt_rule_name(emulator_path)?;
    let registration = qemu_binfmt_registration(&rule_name, emulator_path)
        .with_context(|| format!("unsupported emulator path {emulator_path}"))?;
    fs::write("/proc/sys/fs/binfmt_misc/register", registration)
        .context("failed to register private binfmt_misc handler for foreign-arch command")?;
    Ok(rule_name)
}

fn unregister_private_binfmt(rule_name: &str) -> Result<()> {
    let rule_path = Path::new("/proc/sys/fs/binfmt_misc").join(rule_name);
    if rule_path.exists() {
        fs::write(&rule_path, b"-1").with_context(|| {
            format!(
                "failed to unregister private binfmt_misc handler {}",
                rule_path.display()
            )
        })?;
    }
    Ok(())
}

fn private_binfmt_rule_name(emulator_path: &str) -> Result<String> {
    let basename = Path::new(emulator_path)
        .file_name()
        .and_then(OsStr::to_str)
        .with_context(|| format!("invalid emulator path {emulator_path}"))?;
    let sanitized = basename.replace(|character: char| !character.is_ascii_alphanumeric(), "-");
    Ok(format!("metalor-{sanitized}-{}", std::process::id()))
}

fn qemu_binfmt_registration(rule_name: &str, emulator_path: &str) -> Result<String> {
    let basename = Path::new(emulator_path)
        .file_name()
        .and_then(OsStr::to_str)
        .with_context(|| format!("invalid emulator path {emulator_path}"))?;
    let rule = match basename {
        "qemu-aarch64-static" => {
            ":{name}:M::\\x7fELF\\x02\\x01\\x01\\x00\\x00\\x00\\x00\\x00\\x00\\x00\\x00\\x00\\x02\\x00\\xb7\\x00:\\xff\\xff\\xff\\xff\\xff\\xff\\xff\\x00\\xff\\xff\\xff\\xff\\xff\\xff\\xff\\xff\\xfe\\xff\\xff\\xff:{interpreter}:FP"
        }
        "qemu-riscv64-static" => {
            ":{name}:M::\\x7fELF\\x02\\x01\\x01\\x00\\x00\\x00\\x00\\x00\\x00\\x00\\x00\\x00\\x02\\x00\\xf3\\x00:\\xff\\xff\\xff\\xff\\xff\\xff\\xff\\x00\\xff\\xff\\xff\\xff\\xff\\xff\\xff\\xff\\xfe\\xff\\xff\\xff:{interpreter}:FP"
        }
        "qemu-x86_64-static" => {
            ":{name}:M::\\x7fELF\\x02\\x01\\x01\\x00\\x00\\x00\\x00\\x00\\x00\\x00\\x00\\x00\\x02\\x00\\x3e\\x00:\\xff\\xff\\xff\\xff\\xff\\xff\\xff\\x00\\xff\\xff\\xff\\xff\\xff\\xff\\xff\\xff\\xfe\\xff\\xff\\xff:{interpreter}:FP"
        }
        _ => bail!("unsupported emulator path {emulator_path}"),
    };
    Ok(rule
        .replace("{name}", rule_name)
        .replace("{interpreter}", emulator_path))
}

fn chroot_into(root: &Path) -> Result<()> {
    let root = CString::new(root.as_os_str().as_encoded_bytes())
        .context("container root path contained an interior null")?;
    let result = unsafe { libc_chroot(root.as_ptr()) };
    if result != 0 {
        return Err(std::io::Error::last_os_error()).with_context(|| {
            format!(
                "failed to chroot into {}",
                root.as_c_str().to_string_lossy()
            )
        });
    }
    Ok(())
}

fn chdir_into(path: &str) -> Result<()> {
    let path = CString::new(path).context("container cwd contained an interior null")?;
    let result = unsafe { libc_chdir(path.as_ptr()) };
    if result != 0 {
        return Err(std::io::Error::last_os_error())
            .with_context(|| format!("failed to chdir to {}", path.as_c_str().to_string_lossy()));
    }
    Ok(())
}

fn run_command(command: &mut Command, description: &str) -> Result<()> {
    let output = command
        .output()
        .with_context(|| format!("failed to spawn {description}"))?;
    if !output.status.success() {
        bail!(
            "{} failed: {}{}",
            description,
            String::from_utf8_lossy(&output.stderr),
            String::from_utf8_lossy(&output.stdout)
        );
    }
    Ok(())
}

fn apply_resource_limits(limits: ResourceLimits) -> Result<()> {
    if let Some(cpu_time_seconds) = limits.cpu_time_seconds {
        set_rlimit(
            libc::RLIMIT_CPU,
            cpu_time_seconds,
            "set RLIMIT_CPU for build cell",
        )?;
    }
    if let Some(memory_bytes) = limits.memory_bytes {
        set_rlimit(
            libc::RLIMIT_AS,
            memory_bytes,
            "set RLIMIT_AS for build cell",
        )?;
    }
    if let Some(max_processes) = limits.max_processes {
        set_rlimit(
            libc::RLIMIT_NPROC,
            max_processes,
            "set RLIMIT_NPROC for build cell",
        )?;
    }
    Ok(())
}

fn set_rlimit(resource: libc::__rlimit_resource_t, value: u64, context: &str) -> Result<()> {
    let limit = libc::rlimit {
        rlim_cur: value as libc::rlim_t,
        rlim_max: value as libc::rlim_t,
    };
    let result = unsafe { libc::setrlimit(resource, &limit) };
    if result != 0 {
        return Err(std::io::Error::last_os_error()).with_context(|| context.to_string());
    }
    Ok(())
}

fn unpack_workspace_archive(archive_path: &Path, destination: &Path) -> Result<()> {
    let archive_name = archive_path
        .file_name()
        .and_then(OsStr::to_str)
        .with_context(|| format!("invalid archive path {}", archive_path.display()))?;
    let file = fs::File::open(archive_path).with_context(|| {
        format!(
            "failed to open workspace archive {}",
            archive_path.display()
        )
    })?;
    if archive_name.ends_with(".tar.gz") || archive_name.ends_with(".tgz") {
        let decoder = GzDecoder::new(file);
        unpack_tar_archive(decoder, archive_path, destination)
    } else if archive_name.ends_with(".tar") {
        unpack_tar_archive(file, archive_path, destination)
    } else {
        bail!(
            "unsupported workspace archive format {} (expected .tar, .tar.gz, or .tgz)",
            archive_path.display()
        );
    }
}

fn unpack_tar_archive<R: std::io::Read>(
    reader: R,
    archive_path: &Path,
    destination: &Path,
) -> Result<()> {
    let mut archive = Archive::new(reader);
    for entry in archive
        .entries()
        .with_context(|| format!("failed to read tar archive {}", archive_path.display()))?
    {
        let mut entry = entry
            .with_context(|| format!("failed to read tar entry from {}", archive_path.display()))?;
        let entry_path = entry.path().with_context(|| {
            format!(
                "failed to read tar entry path from {}",
                archive_path.display()
            )
        })?;
        for component in entry_path.components() {
            match component {
                Component::Normal(_) | Component::CurDir => {}
                Component::RootDir | Component::ParentDir | Component::Prefix(_) => {
                    bail!(
                        "workspace archive {} contained an unsafe path {}",
                        archive_path.display(),
                        entry_path.display()
                    );
                }
            }
        }
        let target = destination.join(entry_path.as_ref());
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }
        entry.unpack(&target).with_context(|| {
            format!(
                "failed to unpack {} into {}",
                archive_path.display(),
                target.display()
            )
        })?;
    }
    Ok(())
}

fn copy_directory_contents(source: &Path, destination: &Path) -> Result<()> {
    let metadata = fs::metadata(source)
        .with_context(|| format!("failed to inspect build source {}", source.display()))?;
    if !metadata.is_dir() {
        bail!(
            "build workspace seed must be a directory: {}",
            source.display()
        );
    }
    fs::set_permissions(destination, metadata.permissions())
        .with_context(|| format!("failed to apply permissions to {}", destination.display()))?;
    for entry in fs::read_dir(source)
        .with_context(|| format!("failed to read directory {}", source.display()))?
    {
        let entry =
            entry.with_context(|| format!("failed to read entry in {}", source.display()))?;
        copy_path(&entry.path(), &destination.join(entry.file_name()))?;
    }
    Ok(())
}

fn copy_path(source: &Path, destination: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(source)
        .with_context(|| format!("failed to inspect {}", source.display()))?;
    remove_path_if_exists(destination)?;
    if metadata.file_type().is_symlink() {
        let target = fs::read_link(source)
            .with_context(|| format!("failed to read symlink {}", source.display()))?;
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }
        symlink(&target, destination).with_context(|| {
            format!(
                "failed to create symlink {} -> {}",
                destination.display(),
                target.display()
            )
        })?;
        return Ok(());
    }
    if metadata.is_dir() {
        fs::create_dir_all(destination)
            .with_context(|| format!("failed to create {}", destination.display()))?;
        fs::set_permissions(destination, metadata.permissions())
            .with_context(|| format!("failed to apply permissions to {}", destination.display()))?;
        for entry in fs::read_dir(source)
            .with_context(|| format!("failed to read directory {}", source.display()))?
        {
            let entry =
                entry.with_context(|| format!("failed to read entry in {}", source.display()))?;
            copy_path(&entry.path(), &destination.join(entry.file_name()))?;
        }
        return Ok(());
    }
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    fs::copy(source, destination).with_context(|| {
        format!(
            "failed to copy {} to {}",
            source.display(),
            destination.display()
        )
    })?;
    fs::set_permissions(destination, metadata.permissions())
        .with_context(|| format!("failed to apply permissions to {}", destination.display()))?;
    Ok(())
}

fn remove_path_if_exists(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if metadata.is_dir() && !metadata.file_type().is_symlink() {
                fs::remove_dir_all(path)
                    .with_context(|| format!("failed to remove directory {}", path.display()))?;
            } else {
                fs::remove_file(path)
                    .with_context(|| format!("failed to remove file {}", path.display()))?;
            }
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("failed to inspect {}", path.display())),
    }
}

fn normalize_oci_reference(reference: &str) -> String {
    if reference.contains("://")
        || reference.starts_with("oci:")
        || reference.starts_with("dir:")
        || reference.starts_with("docker-archive:")
        || reference.starts_with("oci-archive:")
        || reference.starts_with("containers-storage:")
    {
        return reference.to_string();
    }
    format!("docker://{reference}")
}

fn normalize_arch_name(arch: &str) -> Result<&'static str> {
    match arch {
        "x86_64" | "amd64" => Ok("x86_64"),
        "aarch64" | "arm64" => Ok("aarch64"),
        "riscv64" => Ok("riscv64"),
        other => bail!("unsupported architecture {other}"),
    }
}

fn requested_oci_arch(requested_arch: Option<&str>) -> Result<Option<&'static str>> {
    requested_arch
        .map(|arch| match normalize_arch_name(arch)? {
            "x86_64" => Ok("amd64"),
            "aarch64" => Ok("arm64"),
            "riscv64" => Ok("riscv64"),
            other => bail!("unsupported OCI architecture {other}"),
        })
        .transpose()
}

fn required_qemu_binary(arch: &str) -> &'static str {
    match arch {
        "x86_64" => "qemu-x86_64-static",
        "aarch64" => "qemu-aarch64-static",
        "riscv64" => "qemu-riscv64-static",
        _ => unreachable!("unsupported normalized architecture"),
    }
}

fn find_tool_in_path(tool: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for directory in std::env::split_paths(&path) {
        let candidate = directory.join(tool);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

fn validate_host_absolute_path(path: &Path, context: &str) -> Result<()> {
    if !path.is_absolute() {
        bail!("{context} must be absolute: {}", path.display());
    }
    for component in path.components() {
        match component {
            Component::RootDir | Component::Normal(_) => {}
            Component::CurDir => {}
            Component::ParentDir => bail!("{context} must not contain '..': {}", path.display()),
            Component::Prefix(_) => bail!("unsupported {context}: {}", path.display()),
        }
    }
    Ok(())
}

fn prepare_host_directory(path: &Path, context: &str) -> Result<PathBuf> {
    validate_host_absolute_path(path, context)?;
    let mut current = PathBuf::from("/");
    for component in path.components() {
        match component {
            Component::RootDir => continue,
            Component::CurDir => continue,
            Component::Normal(part) => {
                current.push(part);
                match fs::symlink_metadata(&current) {
                    Ok(metadata) => {
                        if metadata.file_type().is_symlink() {
                            bail!("{context} {} traverses a symlink", current.display());
                        }
                        if !metadata.is_dir() {
                            bail!(
                                "{context} {} traverses a non-directory path component at {}",
                                path.display(),
                                current.display()
                            );
                        }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                        fs::create_dir(&current).with_context(|| {
                            format!("failed to create {} {}", context, current.display())
                        })?;
                    }
                    Err(error) => {
                        return Err(error).with_context(|| {
                            format!("failed to inspect {} {}", context, current.display())
                        });
                    }
                }
            }
            Component::ParentDir | Component::Prefix(_) => {
                bail!("{context} must be absolute: {}", path.display())
            }
        }
    }
    Ok(path.to_path_buf())
}

fn reject_host_symlink_path(path: &Path, context: &str) -> Result<()> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("failed to inspect {} {}", context, path.display()))?;
    if metadata.file_type().is_symlink() {
        bail!("{context} {} traverses a symlink", path.display());
    }
    Ok(())
}

fn prepare_runtime_root_sentinel_path(root: &Path) -> Result<PathBuf> {
    reject_host_symlink_path(root, "container root")?;
    let sentinel_path = root.join(ROOT_SENTINEL);
    match fs::symlink_metadata(&sentinel_path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() {
                bail!(
                    "runtime root sentinel {} must not be a symlink",
                    sentinel_path.display()
                );
            }
            if !metadata.is_file() {
                bail!(
                    "runtime root sentinel {} must be a file",
                    sentinel_path.display()
                );
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&sentinel_path)
                .with_context(|| {
                    format!(
                        "failed to create runtime root sentinel {}",
                        sentinel_path.display()
                    )
                })?;
        }
        Err(error) => {
            return Err(error).with_context(|| {
                format!(
                    "failed to inspect runtime root sentinel {}",
                    sentinel_path.display()
                )
            });
        }
    }
    Ok(sentinel_path)
}

fn prepare_container_mount_target(
    root: &Path,
    destination: &str,
    expect_directory: bool,
    context: &str,
) -> Result<PathBuf> {
    validate_container_absolute_path(destination, context)?;
    let relative = destination.trim_start_matches('/');
    if relative.is_empty() {
        if expect_directory {
            return Ok(root.to_path_buf());
        }
        bail!("{context} must not be /");
    }

    let mut current = root.to_path_buf();
    reject_host_symlink_path(&current, context)?;
    let components: Vec<_> = Path::new(relative).components().collect();
    for (index, component) in components.iter().enumerate() {
        let Component::Normal(part) = component else {
            bail!("{context} must be absolute: {destination}");
        };
        current.push(part);
        let is_last = index + 1 == components.len();
        match fs::symlink_metadata(&current) {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() {
                    bail!(
                        "{context} {} traverses a symlink at {}",
                        destination,
                        current.display()
                    );
                }
                if is_last {
                    if expect_directory && !metadata.is_dir() {
                        bail!(
                            "{context} {} already exists as a non-directory at {}",
                            destination,
                            current.display()
                        );
                    }
                    if !expect_directory && metadata.is_dir() {
                        bail!(
                            "{context} {} already exists as a directory at {}",
                            destination,
                            current.display()
                        );
                    }
                } else if !metadata.is_dir() {
                    bail!(
                        "{context} {} traverses a non-directory path component at {}",
                        destination,
                        current.display()
                    );
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if is_last {
                    if expect_directory {
                        fs::create_dir(&current).with_context(|| {
                            format!(
                                "failed to create mount target directory {}",
                                current.display()
                            )
                        })?;
                    } else {
                        fs::OpenOptions::new()
                            .create_new(true)
                            .write(true)
                            .open(&current)
                            .with_context(|| {
                                format!("failed to create mount target file {}", current.display())
                            })?;
                    }
                } else {
                    fs::create_dir(&current).with_context(|| {
                        format!(
                            "failed to create intermediate mount target directory {}",
                            current.display()
                        )
                    })?;
                }
            }
            Err(error) => {
                return Err(error).with_context(|| {
                    format!("failed to inspect mount target {}", current.display())
                });
            }
        }
    }
    Ok(current)
}

unsafe extern "C" {
    fn chroot(path: *const std::ffi::c_char) -> i32;
    fn chdir(path: *const std::ffi::c_char) -> i32;
}

unsafe fn libc_chroot(path: *const std::ffi::c_char) -> i32 {
    unsafe { chroot(path) }
}

unsafe fn libc_chdir(path: *const std::ffi::c_char) -> i32 {
    unsafe { chdir(path) }
}
