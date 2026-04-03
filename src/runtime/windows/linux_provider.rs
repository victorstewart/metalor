// Copyright 2026 Victor Stewart
// SPDX-License-Identifier: Apache-2.0

use crate::runtime::linux_provider::{append_output, output_failure_message, ProviderShell};
use anyhow::{bail, Context, Result};
use std::process::{Command, Output};

pub const DEFAULT_WSL_DISTRO: &str = "Ubuntu-24.04";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WslResolution {
    pub distro: String,
    pub auto_install: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WslProvider {
    distro: String,
}

impl WslProvider {
    pub fn new(distro: impl Into<String>) -> Result<Self> {
        let distro = distro.into();
        if distro.trim().is_empty() {
            bail!("WSL distro name must not be empty");
        }
        Ok(Self { distro })
    }

    pub fn distro(&self) -> &str {
        &self.distro
    }

    pub fn ensure_available(&self, auto_install: bool, log: &mut String) -> Result<()> {
        let installed = installed_wsl_distros()?;
        if !installed.iter().any(|installed| installed == self.distro()) {
            if !auto_install {
                bail!(
                    "WSL distro '{}' was not installed; install/configure WSL or enable auto-install",
                    self.distro
                );
            }
            run_host_command(
                "wsl.exe",
                &[
                    "--install",
                    "--distribution",
                    self.distro(),
                    "--no-launch",
                    "--web-download",
                ],
                log,
            )
            .with_context(|| format!("failed to install WSL distro '{}'", self.distro()))?;
        }
        run_host_command(
            "wsl.exe",
            &[
                "--distribution",
                self.distro(),
                "--user",
                "root",
                "--",
                "bash",
                "-lc",
                "true",
            ],
            log,
        )
        .with_context(|| format!("failed to use WSL distro '{}'", self.distro()))?;
        Ok(())
    }
}

impl ProviderShell for WslProvider {
    fn spawn_shell(&self, script: &str) -> Result<Command> {
        let mut command = Command::new("wsl.exe");
        command.args([
            "--distribution",
            self.distro(),
            "--user",
            "root",
            "--",
            "bash",
            "-lc",
            script,
        ]);
        Ok(command)
    }
}

pub fn installed_wsl_distros() -> Result<Vec<String>> {
    let output = Command::new("wsl.exe")
        .args(["--list", "--quiet"])
        .output()
        .context("failed to spawn wsl.exe --list --quiet")?;
    if !output.status.success() {
        bail!(
            "{}",
            output_failure_message("wsl.exe --list --quiet", &output)
        );
    }
    parse_wsl_list_output(&output.stdout)
}

pub fn resolve_wsl_distro(explicit: Option<&str>) -> Result<WslResolution> {
    if let Some(explicit) = explicit {
        let trimmed = explicit.trim();
        if !trimmed.is_empty() {
            return Ok(WslResolution {
                distro: trimmed.to_string(),
                auto_install: false,
            });
        }
    }
    select_wsl_distro(installed_wsl_distros()?)
}

pub fn parse_wsl_list_output(stdout: &[u8]) -> Result<Vec<String>> {
    let decoded = decode_wsl_list_output(stdout)?;
    Ok(decoded
        .lines()
        .map(str::trim)
        .map(|line| line.trim_start_matches('\u{feff}'))
        .filter(|line| !line.is_empty())
        .map(ToOwned::to_owned)
        .collect())
}

fn decode_wsl_list_output(stdout: &[u8]) -> Result<String> {
    if stdout.is_empty() {
        return Ok(String::new());
    }
    let utf16le = stdout.starts_with(&[0xff, 0xfe])
        || stdout.iter().skip(1).step_by(2).any(|byte| *byte == 0);
    if !utf16le {
        return String::from_utf8(stdout.to_vec()).context("wsl.exe output was not valid UTF-8");
    }
    let mut words = Vec::with_capacity(stdout.len().div_ceil(2));
    for chunk in stdout.chunks(2) {
        let low = chunk[0];
        let high = *chunk.get(1).unwrap_or(&0);
        words.push(u16::from_le_bytes([low, high]));
    }
    String::from_utf16(&words).context("wsl.exe output was not valid UTF-16LE")
}

fn select_wsl_distro(installed: Vec<String>) -> Result<WslResolution> {
    if installed
        .iter()
        .any(|installed| installed == DEFAULT_WSL_DISTRO)
    {
        return Ok(WslResolution {
            distro: DEFAULT_WSL_DISTRO.to_string(),
            auto_install: true,
        });
    }
    if let Some(first) = installed.into_iter().next() {
        return Ok(WslResolution {
            distro: first,
            auto_install: false,
        });
    }
    Ok(WslResolution {
        distro: DEFAULT_WSL_DISTRO.to_string(),
        auto_install: true,
    })
}

fn run_host_command(executable: &str, args: &[&str], log: &mut String) -> Result<Output> {
    log.push_str(&format!("run host {} {}\n", executable, args.join(" ")));
    let output = Command::new(executable)
        .args(args)
        .output()
        .with_context(|| format!("failed to spawn {executable}"))?;
    append_output(log, &output);
    if !output.status.success() {
        bail!(
            "{}",
            output_failure_message(&format!("host command '{executable}'"), &output)
        );
    }
    Ok(output)
}
