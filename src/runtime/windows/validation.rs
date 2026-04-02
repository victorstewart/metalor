// Copyright 2026 Victor Stewart
// SPDX-License-Identifier: Apache-2.0

use super::broker::WorkerTarget;
use anyhow::{bail, Result};

pub fn validate_worker_target(target: &WorkerTarget) -> Result<()> {
    if !target.executable.is_absolute() {
        bail!(
            "Windows worker executable must be absolute: {}",
            target.executable.display()
        );
    }
    if let Some(subcommand) = &target.subcommand {
        if subcommand.is_empty() {
            bail!("Windows worker subcommand must not be empty");
        }
    }
    Ok(())
}

pub fn validate_application_id(application_id: &str) -> Result<()> {
    if application_id.is_empty() {
        bail!("application identifier must not be empty");
    }
    if application_id.starts_with('.')
        || application_id.ends_with('.')
        || application_id.contains("..")
    {
        bail!("application identifier must be dot-separated and must not contain empty components: {application_id}");
    }
    for component in application_id.split('.') {
        if !component
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
        {
            bail!(
                "application identifier must contain only ASCII letters, digits, '-', '_', and '.': {application_id}"
            );
        }
    }
    Ok(())
}
