// Copyright 2026 Victor Stewart
// SPDX-License-Identifier: Apache-2.0

use anyhow::{bail, Context, Result};
use sha2::{Digest, Sha256};
use std::ffi::{CString, OsStr};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::ExitStatusExt;
use std::path::Component;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub const RUN_HELPER_DIR: &str = "/.metalor-run";
const ISOLATION_ENV: &str = "METALOR_PRIVATE_NS";
const RUNTIME_PREFIX_ENV: &str = "METALOR_RUNTIME_ROOT_PREFIX";
const ROOT_SENTINEL: &str = ".metalor-root";

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

pub fn helper_binary_path(binary_name: &str) -> String {
    format!("{RUN_HELPER_DIR}/{binary_name}")
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
    let mut process = Command::new("unshare");
    process
        .args(["--fork", "--pid", "--mount", "--uts", "--ipc", "--"])
        .arg(current_exe)
        .arg(subcommand)
        .arg("--root")
        .arg(&canonical_root)
        .arg("--cwd")
        .arg(&command.cwd);
    for mount in &command.mounts {
        process
            .arg("--mount-source")
            .arg(&mount.source)
            .arg("--mount-dest")
            .arg(&mount.destination)
            .arg("--mount-mode")
            .arg(if mount.read_only { "ro" } else { "rw" });
    }
    for (key, value) in &command.env {
        process.arg("--env").arg(format!("{key}={value}"));
    }
    if let Some(emulator) = &command.emulator {
        process.arg("--emulator").arg(emulator);
    }
    process
        .arg("--executable")
        .arg(&command.executable)
        .args(&command.argv)
        .env(ISOLATION_ENV, "1")
        .env(RUNTIME_PREFIX_ENV, canonical_prefix.as_os_str());
    Ok(process)
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
    if !has_mount_covering(command, "/etc/resolv.conf") {
        mount_host_resolv_conf(&command.root)?;
    }
    mount_host_run_devices(command)?;

    chroot_into(&command.root)?;
    let private_binfmt_rule = if let Some(emulator) = &command.emulator {
        Some(register_private_binfmt(emulator)?)
    } else {
        None
    };

    let result = run_container_process(command);
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

fn run_container_process(command: &ContainerRunCommand) -> Result<()> {
    chdir_into(&command.cwd)?;

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
