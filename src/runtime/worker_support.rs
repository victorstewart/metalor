// Copyright 2026 Victor Stewart
// SPDX-License-Identifier: Apache-2.0

use crate::runtime::{BuildCellSpec, CommandSpec, WorkspaceSeed};
use anyhow::{bail, Context, Result};
use flate2::read::GzDecoder;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use tar::Archive;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct WorkerJobLayout {
    pub(crate) root: PathBuf,
}

pub(crate) fn prepare_job_root(job_root: &Path) -> Result<WorkerJobLayout> {
    validate_host_path(job_root, "worker job root")?;
    if job_root.exists() {
        fs::remove_dir_all(job_root)
            .with_context(|| format!("failed to reset worker job root {}", job_root.display()))?;
    }
    fs::create_dir_all(job_root)
        .with_context(|| format!("failed to create worker job root {}", job_root.display()))?;
    Ok(WorkerJobLayout {
        root: job_root.to_path_buf(),
    })
}

pub(crate) fn stage_inputs(spec: &BuildCellSpec, layout: &WorkerJobLayout) -> Result<()> {
    stage_workspace_seed(spec, layout)?;
    for import in &spec.imports {
        copy_path(
            import.source.as_path(),
            &cell_host_path(layout, import.destination.as_str())?,
        )?;
    }
    for cache in &spec.caches {
        let destination = cell_host_path(layout, cache.destination.as_str())?;
        if cache.source.as_path().exists() {
            copy_path(cache.source.as_path(), &destination)?;
        } else {
            fs::create_dir_all(&destination)
                .with_context(|| format!("failed to create {}", destination.display()))?;
        }
    }
    Ok(())
}

pub(crate) fn build_worker_command(
    spec: &BuildCellSpec,
    layout: &WorkerJobLayout,
) -> Result<Command> {
    validate_command_spec(&spec.command)?;
    let cwd = cell_host_path(layout, spec.command.cwd.as_str())?;
    fs::create_dir_all(&cwd).with_context(|| format!("failed to create {}", cwd.display()))?;

    let mut command = Command::new(&spec.command.executable);
    command
        .args(&spec.command.argv)
        .current_dir(cwd)
        .env_clear()
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    for (key, value) in &spec.env {
        command.env(key, value);
    }
    let _ = spec.limits;
    Ok(command)
}

pub(crate) fn sync_caches(spec: &BuildCellSpec, layout: &WorkerJobLayout) -> Result<()> {
    for cache in &spec.caches {
        copy_path(
            &cell_host_path(layout, cache.destination.as_str())?,
            cache.source.as_path(),
        )?;
    }
    Ok(())
}

pub(crate) fn copy_exports(
    spec: &BuildCellSpec,
    layout: &WorkerJobLayout,
    command_succeeded: bool,
) -> Result<()> {
    for export in &spec.exports {
        let source = cell_host_path(layout, export.source.as_str())?;
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

pub(crate) fn cell_host_path(layout: &WorkerJobLayout, cell_path: &str) -> Result<PathBuf> {
    validate_cell_path(cell_path, "build cell path")?;
    let mut host = layout.root.clone();
    let mut saw_component = false;
    for component in cell_path.split('/') {
        match component {
            "" | "." => {}
            ".." => bail!("build cell path must not contain '..': {cell_path}"),
            normal => {
                host.push(normal);
                saw_component = true;
            }
        }
    }
    if !saw_component {
        return Ok(layout.root.clone());
    }
    Ok(host)
}

fn stage_workspace_seed(spec: &BuildCellSpec, layout: &WorkerJobLayout) -> Result<()> {
    let workspace_root = cell_host_path(layout, spec.workspace_path.as_str())?;
    fs::create_dir_all(&workspace_root)
        .with_context(|| format!("failed to create {}", workspace_root.display()))?;
    match &spec.workspace_seed {
        WorkspaceSeed::Empty => Ok(()),
        WorkspaceSeed::SnapshotDir(source) => {
            copy_directory_contents(source.as_path(), &workspace_root)
        }
        WorkspaceSeed::Archive(archive) => {
            unpack_workspace_archive(archive.as_path(), &workspace_root)
        }
    }
}

fn validate_command_spec(command: &CommandSpec) -> Result<()> {
    validate_cell_path(command.cwd.as_str(), "build cell cwd")?;
    validate_executable(&command.executable)?;
    Ok(())
}

fn validate_executable(executable: &str) -> Result<()> {
    if executable.starts_with('/') {
        return validate_cell_path(executable, "build cell executable");
    }
    if executable.is_empty() {
        bail!("build cell executable must not be empty");
    }
    if executable.contains('/') || executable.contains('\\') {
        bail!("build cell executable must be absolute or a bare command name: {executable}");
    }
    if executable == ".." {
        bail!("build cell executable must not contain '..': {executable}");
    }
    Ok(())
}

fn validate_cell_path(path: &str, context: &str) -> Result<()> {
    if !path.starts_with('/') {
        bail!("{context} must be absolute: {path}");
    }
    if path.contains('\\') {
        bail!("{context} must use '/' separators: {path}");
    }
    for component in path.split('/') {
        match component {
            "" | "." => {}
            ".." => bail!("{context} must not contain '..': {path}"),
            _ => {}
        }
    }
    Ok(())
}

fn validate_host_path(path: &Path, context: &str) -> Result<()> {
    if !path.is_absolute() {
        bail!("{context} must be absolute: {}", path.display());
    }
    for component in path.components() {
        match component {
            Component::RootDir
            | Component::Normal(_)
            | Component::CurDir
            | Component::Prefix(_) => {}
            Component::ParentDir => bail!("{context} must not contain '..': {}", path.display()),
        }
    }
    Ok(())
}

fn unpack_workspace_archive(archive_path: &Path, destination: &Path) -> Result<()> {
    let archive_name = archive_path
        .file_name()
        .and_then(|value| value.to_str())
        .with_context(|| format!("invalid archive path {}", archive_path.display()))?;
    let file = fs::File::open(archive_path).with_context(|| {
        format!(
            "failed to open workspace archive {}",
            archive_path.display()
        )
    })?;
    if archive_name.ends_with(".tar.gz") || archive_name.ends_with(".tgz") {
        unpack_tar_archive(GzDecoder::new(file), archive_path, destination)
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
        let target_is_dir = fs::metadata(source)
            .map(|resolved| resolved.is_dir())
            .unwrap_or(false);
        create_symlink(&target, destination, target_is_dir)?;
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

#[cfg(unix)]
fn create_symlink(target: &Path, destination: &Path, _is_dir: bool) -> Result<()> {
    use std::os::unix::fs::symlink;

    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    symlink(target, destination).with_context(|| {
        format!(
            "failed to create symlink {} -> {}",
            destination.display(),
            target.display()
        )
    })?;
    Ok(())
}

#[cfg(windows)]
fn create_symlink(target: &Path, destination: &Path, is_dir: bool) -> Result<()> {
    use std::os::windows::fs::{symlink_dir, symlink_file};

    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    if is_dir {
        symlink_dir(target, destination)
    } else {
        symlink_file(target, destination)
    }
    .with_context(|| {
        format!(
            "failed to create symlink {} -> {}",
            destination.display(),
            target.display()
        )
    })?;
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
