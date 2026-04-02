// Copyright 2026 Victor Stewart
// SPDX-License-Identifier: Apache-2.0

use super::validation::validate_worker_target;
use crate::runtime::{build_cell_request_path, write_build_cell_request, BuildCellSpec};
use anyhow::Result;
use std::path::{Path, PathBuf};
use std::process::Command;

pub const WORKER_REQUEST_ENV: &str = "METALOR_WINDOWS_BUILD_CELL_REQUEST";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkerTarget {
    pub executable: PathBuf,
    pub subcommand: Option<String>,
}

impl WorkerTarget {
    pub fn new(executable: impl Into<PathBuf>, subcommand: Option<String>) -> Result<Self> {
        let target = Self {
            executable: executable.into(),
            subcommand,
        };
        validate_worker_target(&target)?;
        Ok(target)
    }
}

pub fn prepare_worker_request(scratch: &Path, spec: &BuildCellSpec) -> Result<PathBuf> {
    let request_path = build_cell_request_path(scratch);
    write_build_cell_request(&request_path, spec)?;
    Ok(request_path)
}

pub fn build_worker_command(target: &WorkerTarget, request_path: &Path) -> Result<Command> {
    validate_worker_target(target)?;
    let mut command = Command::new(&target.executable);
    if let Some(subcommand) = &target.subcommand {
        command.arg(subcommand);
    }
    command.env(WORKER_REQUEST_ENV, request_path.display().to_string());
    Ok(command)
}

pub fn appcontainer_profile_name(application_id: &str) -> String {
    let mut profile = String::with_capacity(application_id.len());
    for character in application_id.chars() {
        if character.is_ascii_alphanumeric()
            || character == '.'
            || character == '-'
            || character == '_'
        {
            profile.push(character);
        } else {
            profile.push('-');
        }
    }
    profile
}
