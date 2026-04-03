// Copyright 2026 Victor Stewart
// SPDX-License-Identifier: Apache-2.0

use crate::runtime::linux_provider::{append_output, output_failure_message, ProviderShell};
use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

pub const DEFAULT_APPLE_LINUX_BUNDLE: &str = "ubuntu-24.04";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AppleLinuxProvider {
    helper: PathBuf,
    vm_name: String,
}

impl AppleLinuxProvider {
    pub fn new(helper: impl Into<PathBuf>, vm_name: impl Into<String>) -> Result<Self> {
        let provider = Self {
            helper: helper.into(),
            vm_name: vm_name.into(),
        };
        provider.validate()?;
        Ok(provider)
    }

    pub fn helper(&self) -> &Path {
        &self.helper
    }

    pub fn vm_name(&self) -> &str {
        &self.vm_name
    }

    pub fn ensure_available(&self, bundle: &str, log: &mut String) -> Result<()> {
        if bundle.trim().is_empty() {
            bail!("Apple Linux provider bundle name must not be empty");
        }
        run_host_command(
            &self.helper,
            &["ensure", "--vm-name", self.vm_name(), "--bundle", bundle],
            log,
        )
        .with_context(|| {
            format!(
                "failed to use Apple Linux provider helper {}; ensure it can create or resume Linux VM '{}'",
                self.helper.display(),
                self.vm_name()
            )
        })?;
        Ok(())
    }

    fn validate(&self) -> Result<()> {
        if !self.helper.is_absolute() {
            bail!(
                "Apple Linux provider helper must be an absolute path: {}",
                self.helper.display()
            );
        }
        if self.vm_name.trim().is_empty() {
            bail!("Apple Linux provider VM name must not be empty");
        }
        Ok(())
    }
}

impl ProviderShell for AppleLinuxProvider {
    fn spawn_shell(&self, script: &str) -> Result<Command> {
        let mut command = Command::new(self.helper());
        command.args(["shell", "--vm-name", self.vm_name(), "--script", script]);
        Ok(command)
    }
}

fn run_host_command(executable: &Path, args: &[&str], log: &mut String) -> Result<Output> {
    log.push_str(&format!(
        "run host {} {}\n",
        executable.display(),
        args.join(" ")
    ));
    let output = Command::new(executable)
        .args(args)
        .output()
        .with_context(|| format!("failed to spawn {}", executable.display()))?;
    append_output(log, &output);
    if !output.status.success() {
        bail!(
            "{}",
            output_failure_message(&format!("host command '{}'", executable.display()), &output)
        );
    }
    Ok(output)
}
