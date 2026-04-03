// Copyright 2026 Victor Stewart
// SPDX-License-Identifier: Apache-2.0

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::Path;
use std::path::PathBuf;
use std::process::{Command, ExitStatus, Output, Stdio};
use std::str::FromStr;
use std::time::{SystemTime, UNIX_EPOCH};

pub const PROVIDER_METADATA_FILE: &str = "provider-metadata.env";
pub const PROVIDER_RUNTIME_LAYOUT_VERSION: &str = "v1";

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LocalLinuxProviderSelection {
    Auto,
    Wsl2,
    MacLocal,
}

impl LocalLinuxProviderSelection {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Wsl2 => "wsl2",
            Self::MacLocal => "mac-local",
        }
    }
}

impl FromStr for LocalLinuxProviderSelection {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        match value.trim() {
            "" | "auto" => Ok(Self::Auto),
            "wsl2" => Ok(Self::Wsl2),
            "mac-local" => Ok(Self::MacLocal),
            other => bail!(
                "unsupported local Linux provider selection {:?}; expected one of: auto, wsl2, mac-local",
                other
            ),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LocalLinuxProviderKind {
    Wsl2,
    MacLocal,
}

impl LocalLinuxProviderKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Wsl2 => "wsl2",
            Self::MacLocal => "mac-local",
        }
    }
}

impl FromStr for LocalLinuxProviderKind {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        match value.trim() {
            "wsl2" => Ok(Self::Wsl2),
            "mac-local" => Ok(Self::MacLocal),
            other => bail!(
                "unsupported local Linux provider kind {:?}; expected one of: wsl2, mac-local",
                other
            ),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ProviderRuntimeMetadata {
    pub kind: LocalLinuxProviderKind,
    pub identity: String,
    pub runtime_root: String,
    pub runtime_layout_version: String,
    pub bootstrap_version: String,
    pub bootstrap_stamp: String,
}

impl ProviderRuntimeMetadata {
    pub fn validate(&self) -> Result<()> {
        if self.identity.trim().is_empty() {
            bail!("provider_identity must not be empty");
        }
        if self.runtime_root.trim().is_empty() {
            bail!("runtime_root must not be empty");
        }
        if self.runtime_layout_version.trim().is_empty() {
            bail!("runtime_layout_version must not be empty");
        }
        if self.bootstrap_version.trim().is_empty() {
            bail!("bootstrap_version must not be empty");
        }
        if self.bootstrap_stamp.trim().is_empty() {
            bail!("bootstrap_stamp must not be empty");
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProviderRuntimeLayout {
    root: String,
}

impl ProviderRuntimeLayout {
    pub fn new(root: impl Into<String>) -> Result<Self> {
        Ok(Self {
            root: normalize_provider_root(root.into())?,
        })
    }

    pub fn root(&self) -> &str {
        &self.root
    }

    pub fn jobs_root(&self) -> String {
        provider_join(&self.root, "jobs")
    }

    pub fn metadata_path(&self) -> String {
        provider_metadata_path(&self.root)
    }

    pub fn join(&self, relative_path: &str) -> Result<String> {
        validate_provider_relative_path(relative_path, "provider runtime relative path")?;
        Ok(provider_join(&self.root, relative_path))
    }

    pub fn stamp_path(&self, label: &str, version: &str) -> Result<String> {
        validate_stamp_component(label, "provider stamp label")?;
        validate_stamp_component(version, "provider stamp version")?;
        Ok(provider_join(
            &self.root,
            &format!("{label}-{version}.stamp"),
        ))
    }

    pub fn metadata(
        &self,
        kind: LocalLinuxProviderKind,
        identity: impl Into<String>,
        bootstrap_version: &str,
    ) -> Result<ProviderRuntimeMetadata> {
        let metadata = ProviderRuntimeMetadata {
            kind,
            identity: identity.into(),
            runtime_root: self.root.clone(),
            runtime_layout_version: PROVIDER_RUNTIME_LAYOUT_VERSION.to_string(),
            bootstrap_version: bootstrap_version.to_string(),
            bootstrap_stamp: self.stamp_path("bootstrap", bootstrap_version)?,
        };
        metadata.validate()?;
        Ok(metadata)
    }
}

pub fn provider_metadata_path(runtime_root: impl AsRef<str>) -> String {
    let runtime_root = runtime_root.as_ref();
    if runtime_root == "/" {
        format!("/{PROVIDER_METADATA_FILE}")
    } else {
        let runtime_root = runtime_root.trim_end_matches('/');
        if runtime_root.is_empty() {
            PROVIDER_METADATA_FILE.to_string()
        } else {
            format!("{runtime_root}/{PROVIDER_METADATA_FILE}")
        }
    }
}

pub fn render_provider_metadata_env(metadata: &ProviderRuntimeMetadata) -> Result<String> {
    metadata.validate()?;
    Ok(format!(
        "provider_kind={kind}\nprovider_identity={identity}\nruntime_root={runtime_root}\nruntime_layout_version={runtime_layout_version}\nbootstrap_version={bootstrap_version}\nbootstrap_stamp={bootstrap_stamp}\n",
        kind = metadata.kind.as_str(),
        identity = metadata.identity,
        runtime_root = metadata.runtime_root,
        runtime_layout_version = metadata.runtime_layout_version,
        bootstrap_version = metadata.bootstrap_version,
        bootstrap_stamp = metadata.bootstrap_stamp,
    ))
}

pub fn parse_provider_metadata_env(contents: &str) -> Result<ProviderRuntimeMetadata> {
    let mut values = BTreeMap::<&str, &str>::new();
    for raw_line in contents.lines() {
        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }
        let (key, value) = line.split_once('=').ok_or_else(|| {
            anyhow!(
                "invalid provider metadata line {:?}: expected key=value",
                line
            )
        })?;
        values.insert(key.trim(), value);
    }
    let metadata = ProviderRuntimeMetadata {
        kind: required_provider_kind(&values)?,
        identity: required_provider_value(&values, "provider_identity")?.to_string(),
        runtime_root: required_provider_value(&values, "runtime_root")?.to_string(),
        runtime_layout_version: required_provider_value(&values, "runtime_layout_version")?
            .to_string(),
        bootstrap_version: required_provider_value(&values, "bootstrap_version")?.to_string(),
        bootstrap_stamp: required_provider_value(&values, "bootstrap_stamp")?.to_string(),
    };
    metadata.validate()?;
    Ok(metadata)
}

pub fn write_provider_metadata_env(
    path: impl AsRef<Path>,
    metadata: &ProviderRuntimeMetadata,
) -> Result<()> {
    let path = path.as_ref();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let payload = render_provider_metadata_env(metadata)?;
    fs::write(path, payload)
        .with_context(|| format!("failed to write provider metadata {}", path.display()))?;
    Ok(())
}

pub fn read_provider_metadata_env(path: impl AsRef<Path>) -> Result<ProviderRuntimeMetadata> {
    let path = path.as_ref();
    let payload = fs::read_to_string(path)
        .with_context(|| format!("failed to read provider metadata {}", path.display()))?;
    parse_provider_metadata_env(&payload)
        .with_context(|| format!("failed to parse provider metadata {}", path.display()))
}

pub trait ProviderShell {
    fn spawn_shell(&self, script: &str) -> Result<Command>;
}

impl<F> ProviderShell for F
where
    F: Fn(&str) -> Result<Command>,
{
    fn spawn_shell(&self, script: &str) -> Result<Command> {
        (self)(script)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RemoteJobRoot {
    pub root: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WarmState {
    Cold,
    Warm,
}

#[derive(Clone, Debug)]
pub struct ProviderSession<S> {
    shell: S,
}

impl<S> ProviderSession<S> {
    pub fn new(shell: S) -> Self {
        Self { shell }
    }
}

impl<S> ProviderSession<S>
where
    S: ProviderShell,
{
    pub fn run(&self, script: &str, log: &mut String) -> Result<()> {
        let output = self.capture(script, log)?;
        if !output.status.success() {
            bail!(
                "{}",
                shell_failure_message(output.status, &output.stdout, &output.stderr)
            );
        }
        Ok(())
    }

    pub fn path_exists(&self, path: &str) -> Result<bool> {
        let output = self
            .shell
            .spawn_shell(&format!(
                "if [ -e {path} ]; then printf 1; else printf 0; fi",
                path = shell_quote(path)
            ))?
            .output()
            .context("failed to spawn provider shell")?;
        if !output.status.success() {
            bail!(
                "{}",
                shell_failure_message(output.status, &output.stdout, &output.stderr)
            );
        }
        Ok(output.stdout.starts_with(b"1"))
    }

    pub fn prepare_job_root(
        &self,
        runtime: &ProviderRuntimeLayout,
        label: &str,
        log: &mut String,
    ) -> Result<RemoteJobRoot> {
        let label = sanitize_job_label(label);
        let jobs_root = runtime.jobs_root();
        let root = provider_join(
            &jobs_root,
            &format!(
                "{label}-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos()
            ),
        );
        self.run(
            &format!(
                "mkdir -p {jobs_root} && rm -rf {root} && mkdir -p {root}",
                jobs_root = shell_quote(&jobs_root),
                root = shell_quote(&root),
            ),
            log,
        )?;
        log.push_str(&format!("provider job root: {root}\n"));
        Ok(RemoteJobRoot { root })
    }

    pub fn write_runtime_metadata(
        &self,
        metadata: &ProviderRuntimeMetadata,
        log: &mut String,
    ) -> Result<()> {
        let payload = render_provider_metadata_env(metadata)?;
        self.run(
            &format!(
                "mkdir -p {root} && cat > {path} <<'EOF'\n{payload}EOF\n",
                root = shell_quote(&metadata.runtime_root),
                path = shell_quote(&provider_metadata_path(&metadata.runtime_root)),
                payload = payload,
            ),
            log,
        )
    }

    pub fn ensure_warm_state<F>(
        &self,
        label: &str,
        stamp_path: &str,
        required_paths: &[&str],
        log: &mut String,
        cold: F,
    ) -> Result<WarmState>
    where
        F: FnOnce(&Self, &mut String) -> Result<()>,
    {
        if label.trim().is_empty() {
            bail!("warm-state label must not be empty");
        }
        let mut warm = self.path_exists(stamp_path)?;
        for path in required_paths {
            if !warm {
                break;
            }
            warm = self.path_exists(path)?;
        }
        log.push_str(&format!(
            "provider {label}: {}\n",
            if warm { "warm" } else { "cold" }
        ));
        if warm {
            return Ok(WarmState::Warm);
        }
        cold(self, log)?;
        self.touch_path(stamp_path, log)?;
        Ok(WarmState::Cold)
    }

    pub fn stage_host_path(
        &self,
        local_path: &Path,
        remote_parent: &str,
        restore_executable_bins: bool,
        log: &mut String,
    ) -> Result<String> {
        let local_path = canonical_host_path(local_path)?;
        let parent = local_path
            .parent()
            .ok_or_else(|| anyhow!("path has no parent: {}", local_path.display()))?;
        let name = file_name_string(&local_path)?;
        let remote_root = provider_join(remote_parent, &name);
        let provider_script = if restore_executable_bins {
            format!(
                "mkdir -p {parent} && tar -xf - -C {parent} && if [ -d {root}/bin ]; then find {root}/bin -type f -exec chmod a+rx {{}} +; fi",
                parent = shell_quote(remote_parent),
                root = shell_quote(&remote_root),
            )
        } else {
            format!(
                "mkdir -p {parent} && tar -xf - -C {parent}",
                parent = shell_quote(remote_parent),
            )
        };
        pipe_host_tar_to_shell(
            &[
                "-cf".to_string(),
                "-".to_string(),
                "-C".to_string(),
                normalize_host_path(parent).display().to_string(),
                name,
            ],
            &provider_script,
            |script| self.shell.spawn_shell(script),
            log,
        )?;
        Ok(remote_root)
    }

    pub fn collect_path(
        &self,
        remote_path: &str,
        local_parent: &Path,
        log: &mut String,
    ) -> Result<()> {
        fs::create_dir_all(local_parent)
            .with_context(|| format!("failed to create {}", local_parent.display()))?;
        let remote_parent = Path::new(remote_path)
            .parent()
            .ok_or_else(|| anyhow!("remote path has no parent: {remote_path}"))?
            .display()
            .to_string();
        let remote_name = Path::new(remote_path)
            .file_name()
            .and_then(|value| value.to_str())
            .ok_or_else(|| anyhow!("remote path must have a normal file name: {remote_path}"))?
            .to_string();
        pipe_shell_tar_to_host(
            &format!(
                "tar -cf - -C {parent} {name}",
                parent = shell_quote(&remote_parent),
                name = shell_quote(&remote_name),
            ),
            &[
                "-xf".to_string(),
                "-".to_string(),
                "-C".to_string(),
                normalize_host_path(local_parent).display().to_string(),
            ],
            |script| self.shell.spawn_shell(script),
            log,
        )
    }

    pub fn remove_path(&self, remote_path: &str, log: &mut String) -> Result<()> {
        self.run(&format!("rm -rf {}", shell_quote(remote_path)), log)
    }

    fn touch_path(&self, path: &str, log: &mut String) -> Result<()> {
        let parent = Path::new(path)
            .parent()
            .ok_or_else(|| anyhow!("provider path has no parent: {path}"))?
            .display()
            .to_string();
        self.run(
            &format!(
                "mkdir -p {parent} && touch {path}",
                parent = shell_quote(&parent),
                path = shell_quote(path),
            ),
            log,
        )
    }

    fn capture(&self, script: &str, log: &mut String) -> Result<Output> {
        let output = self
            .shell
            .spawn_shell(script)?
            .output()
            .context("failed to spawn provider shell")?;
        append_output(log, &output);
        Ok(output)
    }
}

fn required_provider_kind(values: &BTreeMap<&str, &str>) -> Result<LocalLinuxProviderKind> {
    LocalLinuxProviderKind::from_str(required_provider_value(values, "provider_kind")?)
}

fn required_provider_value<'a>(values: &'a BTreeMap<&str, &str>, key: &str) -> Result<&'a str> {
    values
        .get(key)
        .copied()
        .ok_or_else(|| anyhow!("missing required provider metadata key {}", key))
}

fn canonical_host_path(path: &Path) -> Result<PathBuf> {
    fs::canonicalize(path).with_context(|| format!("failed to access {}", path.display()))
}

fn normalize_provider_root(root: String) -> Result<String> {
    let trimmed = root.trim();
    if trimmed.is_empty() {
        bail!("provider runtime root must not be empty");
    }
    if !trimmed.starts_with('/') {
        bail!("provider runtime root must be an absolute Linux path: {trimmed}");
    }
    if trimmed.contains('\\') {
        bail!("provider runtime root must use '/' separators: {trimmed}");
    }
    for component in trimmed.split('/') {
        match component {
            "" => {}
            "." | ".." => {
                bail!("provider runtime root must not contain '.' or '..': {trimmed}");
            }
            _ => {}
        }
    }
    if trimmed == "/" {
        Ok("/".to_string())
    } else {
        Ok(trimmed.trim_end_matches('/').to_string())
    }
}

fn validate_provider_relative_path(path: &str, context: &str) -> Result<()> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        bail!("{context} must not be empty");
    }
    if trimmed.starts_with('/') {
        bail!("{context} must be relative: {trimmed}");
    }
    if trimmed.contains('\\') {
        bail!("{context} must use '/' separators: {trimmed}");
    }
    for component in trimmed.split('/') {
        match component {
            "" | "." | ".." => {
                bail!("{context} must not contain empty, '.' or '..' components: {trimmed}")
            }
            _ => {}
        }
    }
    Ok(())
}

fn validate_stamp_component(value: &str, context: &str) -> Result<()> {
    if value.is_empty() {
        bail!("{context} must not be empty");
    }
    if !value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
    {
        bail!("{context} must contain only ASCII letters, digits, '-', '_', and '.': {value}");
    }
    Ok(())
}

fn sanitize_job_label(label: &str) -> String {
    let sanitized: String = label
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.') {
                ch
            } else {
                '-'
            }
        })
        .collect();
    if sanitized.is_empty() {
        "job".to_string()
    } else {
        sanitized
    }
}

fn provider_join(parent: &str, child: &str) -> String {
    if parent == "/" {
        format!("/{child}")
    } else {
        format!("{}/{}", parent.trim_end_matches('/'), child)
    }
}

#[cfg(target_os = "windows")]
fn normalize_host_path(path: &Path) -> PathBuf {
    let raw = path.to_string_lossy();
    if let Some(stripped) = raw.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{stripped}"));
    }
    if let Some(stripped) = raw.strip_prefix(r"\\?\") {
        return PathBuf::from(stripped);
    }
    path.to_path_buf()
}

#[cfg(not(target_os = "windows"))]
fn normalize_host_path(path: &Path) -> PathBuf {
    path.to_path_buf()
}

fn pipe_host_tar_to_shell<F>(
    tar_args: &[String],
    provider_script: &str,
    spawn_shell: F,
    log: &mut String,
) -> Result<()>
where
    F: FnOnce(&str) -> Result<Command>,
{
    log.push_str(&format!(
        "push into provider with tar {}\n",
        tar_args.join(" ")
    ));
    let mut producer = Command::new("tar");
    producer
        .args(tar_args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut consumer = spawn_shell(provider_script)?;
    consumer
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    pipe_commands("tar producer", producer, "provider extract", consumer, log)
}

fn pipe_shell_tar_to_host<F>(
    provider_script: &str,
    tar_args: &[String],
    spawn_shell: F,
    log: &mut String,
) -> Result<()>
where
    F: FnOnce(&str) -> Result<Command>,
{
    log.push_str(&format!(
        "pull from provider with tar {}\n",
        tar_args.join(" ")
    ));
    let mut producer = spawn_shell(provider_script)?;
    producer.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut consumer = Command::new("tar");
    consumer
        .args(tar_args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    pipe_commands("provider tar", producer, "host tar", consumer, log)
}

fn pipe_commands(
    producer_label: &str,
    mut producer: Command,
    consumer_label: &str,
    mut consumer: Command,
    log: &mut String,
) -> Result<()> {
    let mut producer_child = producer
        .spawn()
        .with_context(|| format!("failed to spawn {producer_label}"))?;
    let mut consumer_child = consumer
        .spawn()
        .with_context(|| format!("failed to spawn {consumer_label}"))?;
    {
        let mut producer_stdout = producer_child
            .stdout
            .take()
            .ok_or_else(|| anyhow!("{producer_label} did not expose stdout"))?;
        let mut consumer_stdin = consumer_child
            .stdin
            .take()
            .ok_or_else(|| anyhow!("{consumer_label} did not expose stdin"))?;
        let copy_result = io::copy(&mut producer_stdout, &mut consumer_stdin);
        drop(consumer_stdin);
        drop(producer_stdout);
        let producer_output = producer_child
            .wait_with_output()
            .with_context(|| format!("failed to wait for {producer_label}"))?;
        let consumer_output = consumer_child
            .wait_with_output()
            .with_context(|| format!("failed to wait for {consumer_label}"))?;
        append_output(log, &producer_output);
        append_output(log, &consumer_output);
        if !producer_output.status.success() {
            bail!(
                "{}",
                prefixed_output_failure_message(producer_label, &producer_output)
            );
        }
        if !consumer_output.status.success() {
            bail!(
                "{}",
                prefixed_output_failure_message(consumer_label, &consumer_output)
            );
        }
        if let Err(error) = copy_result {
            if error.kind() != io::ErrorKind::BrokenPipe {
                return Err(error).with_context(|| {
                    format!("failed to pipe {producer_label} into {consumer_label}")
                });
            }
        }
        return Ok(());
    }
}

fn shell_quote(value: &str) -> String {
    let escaped = value.replace('\'', "'\"'\"'");
    format!("'{escaped}'")
}

fn file_name_string(path: &Path) -> Result<String> {
    path.file_name()
        .and_then(|value| value.to_str())
        .map(ToOwned::to_owned)
        .ok_or_else(|| anyhow!("path must have a normal file name: {}", path.display()))
}

pub(crate) fn append_output(log: &mut String, output: &Output) {
    if !output.stdout.is_empty() {
        log.push_str(&String::from_utf8_lossy(&output.stdout));
    }
    if !output.stderr.is_empty() {
        log.push_str(&String::from_utf8_lossy(&output.stderr));
    }
}

fn shell_failure_message(status: ExitStatus, stdout: &[u8], stderr: &[u8]) -> String {
    prefixed_failure_message("provider shell command", status, stdout, stderr)
}

pub(crate) fn output_failure_message(prefix: &str, output: &Output) -> String {
    prefixed_failure_message(prefix, output.status, &output.stdout, &output.stderr)
}

fn prefixed_output_failure_message(prefix: &str, output: &Output) -> String {
    output_failure_message(prefix, output)
}

fn prefixed_failure_message(
    prefix: &str,
    status: ExitStatus,
    stdout: &[u8],
    stderr: &[u8],
) -> String {
    let mut message = format!("{prefix} failed with status {status}");
    append_process_failure_output(&mut message, "stdout", stdout);
    append_process_failure_output(&mut message, "stderr", stderr);
    message
}

fn append_process_failure_output(message: &mut String, label: &str, bytes: &[u8]) {
    if bytes.is_empty() {
        return;
    }
    message.push_str(&format!(
        "\n{label}: {}",
        String::from_utf8_lossy(bytes).trim_end()
    ));
}
