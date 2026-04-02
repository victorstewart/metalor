// Copyright 2026 Victor Stewart
// SPDX-License-Identifier: Apache-2.0

use super::client::HelperTarget;
use anyhow::{bail, Result};

pub fn validate_helper_target(target: &HelperTarget) -> Result<()> {
    validate_bundle_identifier(&target.bundle_identifier)?;
    validate_service_name(&target.service_name)?;
    if !target
        .service_name
        .starts_with(&format!("{}.", target.bundle_identifier))
    {
        bail!(
            "service name {} must start with bundle identifier {}.",
            target.service_name,
            target.bundle_identifier
        );
    }
    Ok(())
}

pub fn validate_bundle_identifier(identifier: &str) -> Result<()> {
    validate_dot_identifier(identifier, "bundle identifier")
}

pub fn validate_service_name(service_name: &str) -> Result<()> {
    validate_dot_identifier(service_name, "service name")
}

fn validate_dot_identifier(value: &str, context: &str) -> Result<()> {
    if value.is_empty() {
        bail!("{context} must not be empty");
    }
    if value.starts_with('.') || value.ends_with('.') || value.contains("..") {
        bail!("{context} must be dot-separated and must not contain empty components: {value}");
    }
    for component in value.split('.') {
        if component.is_empty() {
            bail!("{context} must not contain empty components: {value}");
        }
        if !component
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
        {
            bail!("{context} must contain only ASCII letters, digits, '-', '_', and '.': {value}");
        }
    }
    Ok(())
}
