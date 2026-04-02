// Copyright 2026 Victor Stewart
// SPDX-License-Identifier: Apache-2.0

use super::portable::BuildCellSpec;
use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

const BUILD_CELL_REQUEST_FILE: &str = "build-cell-request.json";

pub fn build_cell_request_path(scratch: &Path) -> PathBuf {
    scratch.join(BUILD_CELL_REQUEST_FILE)
}

pub fn write_build_cell_request(path: &Path, spec: &BuildCellSpec) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let payload =
        serde_json::to_vec_pretty(spec).context("failed to serialize build cell request")?;
    fs::write(path, payload)
        .with_context(|| format!("failed to write build cell request {}", path.display()))?;
    Ok(())
}

pub fn read_build_cell_request(path: &Path) -> Result<BuildCellSpec> {
    let payload = fs::read(path)
        .with_context(|| format!("failed to read build cell request {}", path.display()))?;
    serde_json::from_slice(&payload).context("failed to parse build cell request")
}
