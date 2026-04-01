// Copyright 2026 Victor Stewart
// SPDX-License-Identifier: Apache-2.0

use anyhow::Result;
use metalor::{
    build_unshare_reexec_command, interpolate_braced_variables, parse_exec_array,
    prepare_oci_rootfs, prepare_runtime_emulator, run_isolated_container_command,
    significant_lines, valid_identifier, BindMount, ContainerRunCommand, SignificantLine,
};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

fn assert_build_unshare_reexec_command_signature(
    _function: fn(&Path, &str, &Path, &ContainerRunCommand) -> Result<Command>,
) {
}

fn assert_prepare_runtime_emulator_signature(
    _function: fn(&Path, &str, &str) -> Result<Option<String>>,
) {
}

fn assert_prepare_oci_rootfs_signature(
    _function: fn(&str, &Path, &Path, Option<&Path>, Option<&str>) -> Result<PathBuf>,
) {
}

fn assert_run_isolated_container_command_signature(
    _function: fn(&ContainerRunCommand) -> Result<()>,
) {
}

#[test]
fn crates_io_readme_parser_example_works_as_claimed() -> Result<()> {
    let line = significant_lines(
        r#"
        # comment
        RUN ["sh", "-lc", "echo ${TARGET}"]
        "#,
    )
    .next()
    .unwrap();

    assert_eq!(line.number, 3);

    let argv = parse_exec_array(r#"["sh", "-lc", "echo ${TARGET}"]"#)?;
    let mut vars = BTreeMap::new();
    vars.insert("TARGET".to_string(), "world".to_string());

    let expanded = interpolate_braced_variables(&argv[2], &vars, "ARG")?;
    assert_eq!(expanded, "echo world");
    Ok(())
}

#[test]
fn github_readme_public_api_at_a_glance_is_exported() {
    let _line = SignificantLine {
        number: 1,
        text: "RUN true",
    };
    let _bind_mount = BindMount {
        source: PathBuf::from("/tmp/source"),
        destination: "/tmp/destination".to_string(),
        read_only: true,
    };
    let request = ContainerRunCommand {
        root: PathBuf::from("/tmp/root"),
        cwd: "/".to_string(),
        mounts: Vec::new(),
        env: Vec::new(),
        emulator: None,
        executable: "/bin/true".to_string(),
        argv: Vec::new(),
    };

    assert!(valid_identifier("TARGET"));
    assert_eq!(
        significant_lines("RUN true").next(),
        Some(SignificantLine {
            number: 1,
            text: "RUN true",
        })
    );

    assert_build_unshare_reexec_command_signature(build_unshare_reexec_command);
    assert_prepare_runtime_emulator_signature(prepare_runtime_emulator);
    assert_prepare_oci_rootfs_signature(prepare_oci_rootfs);
    assert_run_isolated_container_command_signature(run_isolated_container_command);

    let _ = request;
}

#[test]
fn readmes_supported_architecture_aliases_are_accepted() {
    for (host_arch, guest_arch) in [
        ("x86_64", "amd64"),
        ("amd64", "x86_64"),
        ("aarch64", "arm64"),
        ("arm64", "aarch64"),
        ("riscv64", "riscv64"),
    ] {
        assert_eq!(
            prepare_runtime_emulator(Path::new("/"), host_arch, guest_arch).unwrap(),
            None
        );
    }
}
