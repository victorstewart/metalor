// Copyright 2026 Victor Stewart
// SPDX-License-Identifier: Apache-2.0

use super::broker::WORKER_REQUEST_ENV;
use crate::runtime::worker_support::{
    build_worker_command as build_shared_worker_command, copy_exports, prepare_job_root,
    stage_inputs, sync_caches, WorkerJobLayout,
};
use crate::runtime::{read_build_cell_request, BuildCellSpec};
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkerJob {
    pub root: PathBuf,
}

pub fn load_request(request_path: &Path) -> Result<BuildCellSpec> {
    read_build_cell_request(request_path)
}

pub fn load_request_from_env() -> Result<BuildCellSpec> {
    let request = std::env::var(WORKER_REQUEST_ENV)
        .context("missing required Windows worker request environment variable")?;
    load_request(Path::new(&request))
}

pub fn prepare_job(job_root: &Path, spec: &BuildCellSpec) -> Result<WorkerJob> {
    let layout = prepare_job_root(job_root)?;
    stage_inputs(spec, &layout)?;
    Ok(WorkerJob { root: layout.root })
}

pub fn build_worker_process_command(spec: &BuildCellSpec, job: &WorkerJob) -> Result<Command> {
    build_shared_worker_command(
        spec,
        &WorkerJobLayout {
            root: job.root.clone(),
        },
    )
}

pub fn sync_worker_caches(spec: &BuildCellSpec, job: &WorkerJob) -> Result<()> {
    sync_caches(
        spec,
        &WorkerJobLayout {
            root: job.root.clone(),
        },
    )
}

pub fn copy_worker_exports(
    spec: &BuildCellSpec,
    job: &WorkerJob,
    command_succeeded: bool,
) -> Result<()> {
    copy_exports(
        spec,
        &WorkerJobLayout {
            root: job.root.clone(),
        },
        command_succeeded,
    )
}
