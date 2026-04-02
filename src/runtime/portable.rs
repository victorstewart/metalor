// Copyright 2026 Victor Stewart
// SPDX-License-Identifier: Apache-2.0

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct HostPath(PathBuf);

impl HostPath {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self(path.into())
    }

    pub fn as_path(&self) -> &Path {
        &self.0
    }

    pub fn into_path_buf(self) -> PathBuf {
        self.0
    }
}

impl AsRef<Path> for HostPath {
    fn as_ref(&self) -> &Path {
        self.as_path()
    }
}

impl From<PathBuf> for HostPath {
    fn from(path: PathBuf) -> Self {
        Self(path)
    }
}

impl From<&Path> for HostPath {
    fn from(path: &Path) -> Self {
        Self(path.to_path_buf())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CellPath(String);

impl CellPath {
    pub fn new(path: impl Into<String>) -> Self {
        Self(path.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_string(self) -> String {
        self.0
    }
}

impl From<String> for CellPath {
    fn from(path: String) -> Self {
        Self(path)
    }
}

impl From<&str> for CellPath {
    fn from(path: &str) -> Self {
        Self(path.to_string())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum WorkspaceSeed {
    Empty,
    SnapshotDir(HostPath),
    Archive(HostPath),
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ImportSpec {
    pub source: HostPath,
    pub destination: CellPath,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CacheSpec {
    pub source: HostPath,
    pub destination: CellPath,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ExportSpec {
    pub source: CellPath,
    pub destination: HostPath,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CommandSpec {
    pub cwd: CellPath,
    pub executable: String,
    #[serde(default)]
    pub argv: Vec<String>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub enum NetworkPolicy {
    #[default]
    Enabled,
    Disabled,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct ResourceLimits {
    pub cpu_time_seconds: Option<u64>,
    pub memory_bytes: Option<u64>,
    pub max_processes: Option<u64>,
}

impl ResourceLimits {
    pub fn is_unbounded(&self) -> bool {
        self.cpu_time_seconds.is_none()
            && self.memory_bytes.is_none()
            && self.max_processes.is_none()
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub enum CleanupPolicy {
    #[default]
    Always,
    PreserveOnFailure,
    Never,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct BuildCellSpec {
    pub root: HostPath,
    pub scratch: HostPath,
    pub workspace_path: CellPath,
    pub workspace_seed: WorkspaceSeed,
    #[serde(default)]
    pub imports: Vec<ImportSpec>,
    #[serde(default)]
    pub caches: Vec<CacheSpec>,
    #[serde(default)]
    pub exports: Vec<ExportSpec>,
    pub command: CommandSpec,
    #[serde(default)]
    pub env: Vec<(String, String)>,
    #[serde(default)]
    pub network: NetworkPolicy,
    #[serde(default)]
    pub limits: ResourceLimits,
    #[serde(default)]
    pub cleanup: CleanupPolicy,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct BuildCellResult {
    pub scratch_preserved: bool,
}
