// Copyright 2026 Victor Stewart
// SPDX-License-Identifier: Apache-2.0

use super::validation::validate_helper_target;
use crate::runtime::{build_cell_request_path, write_build_cell_request, BuildCellSpec};
use anyhow::Result;
use std::path::{Path, PathBuf};

pub const HELPER_REQUEST_ENV: &str = "METALOR_MACOS_BUILD_CELL_REQUEST";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HelperTarget {
    pub bundle_identifier: String,
    pub service_name: String,
}

impl HelperTarget {
    pub fn new(
        bundle_identifier: impl Into<String>,
        service_name: impl Into<String>,
    ) -> Result<Self> {
        let target = Self {
            bundle_identifier: bundle_identifier.into(),
            service_name: service_name.into(),
        };
        validate_helper_target(&target)?;
        Ok(target)
    }

    pub fn xpc_bundle_name(&self) -> String {
        format!("{}.xpc", self.service_name)
    }

    pub fn bundle_relative_path(&self) -> PathBuf {
        PathBuf::from("Contents")
            .join("XPCServices")
            .join(self.xpc_bundle_name())
    }

    pub fn executable_relative_path(&self) -> PathBuf {
        self.bundle_relative_path()
            .join("Contents")
            .join("MacOS")
            .join(self.executable_name())
    }

    pub fn executable_name(&self) -> &str {
        self.service_name
            .rsplit('.')
            .next()
            .unwrap_or(self.service_name.as_str())
    }
}

pub fn prepare_helper_request(scratch: &Path, spec: &BuildCellSpec) -> Result<PathBuf> {
    let request_path = build_cell_request_path(scratch);
    write_build_cell_request(&request_path, spec)?;
    Ok(request_path)
}

pub fn helper_environment(request_path: &Path) -> Vec<(String, String)> {
    vec![(
        HELPER_REQUEST_ENV.to_string(),
        request_path.display().to_string(),
    )]
}
