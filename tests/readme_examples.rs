// Copyright 2026 Victor Stewart
// SPDX-License-Identifier: Apache-2.0

use anyhow::Result;
use metalor::{
    build_cell_request_path, interpolate_braced_variables, parse_exec_array, significant_lines,
    valid_identifier, BackendCaps, BuildCellSpec, CacheSpec, CellPath, CleanupPolicy, CommandSpec,
    ExportSpec, HostPath, ImportSpec, NetworkPolicy, ResourceLimits, SignificantLine,
    WorkspaceSeed,
};
use std::collections::BTreeMap;
use std::path::PathBuf;

#[cfg(target_os = "linux")]
use metalor::{
    build_cell_reexec_command, build_unshare_reexec_command, finalize_build_cell,
    prepare_oci_rootfs, prepare_runtime_emulator, run_build_cell, run_isolated_container_command,
    BindMount, BuildCellResult, ContainerRunCommand,
};
#[cfg(target_os = "linux")]
use std::path::Path;
#[cfg(target_os = "linux")]
use std::process::Command;

#[cfg(target_os = "linux")]
fn assert_build_unshare_reexec_command_signature(
    _function: fn(&Path, &str, &Path, &ContainerRunCommand) -> Result<Command>,
) {
}

#[cfg(target_os = "linux")]
fn assert_prepare_runtime_emulator_signature(
    _function: fn(&Path, &str, &str) -> Result<Option<String>>,
) {
}

#[cfg(target_os = "linux")]
fn assert_prepare_oci_rootfs_signature(
    _function: fn(&str, &Path, &Path, Option<&Path>, Option<&str>) -> Result<PathBuf>,
) {
}

#[cfg(target_os = "linux")]
fn assert_run_isolated_container_command_signature(
    _function: fn(&ContainerRunCommand) -> Result<()>,
) {
}

#[cfg(target_os = "linux")]
fn assert_build_cell_reexec_command_signature(
    _function: fn(&Path, &str, &Path, &BuildCellSpec) -> Result<Command>,
) {
}

#[cfg(target_os = "linux")]
fn assert_run_build_cell_signature(_function: fn(&BuildCellSpec) -> Result<()>) {}

#[cfg(target_os = "linux")]
fn assert_finalize_build_cell_signature(
    _function: fn(&BuildCellSpec, bool) -> Result<BuildCellResult>,
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
#[cfg(target_os = "linux")]
fn github_readme_public_api_at_a_glance_is_exported() {
    let _line = SignificantLine {
        number: 1,
        text: "RUN true",
    };
    let _backend_caps = BackendCaps {
        oci_rootfs: true,
        live_bind_mounts: true,
        foreign_arch_exec: true,
        per_job_network_toggle: true,
        profile_selected_network: false,
    };
    let _bind_mount = BindMount {
        source: PathBuf::from("/tmp/source"),
        destination: "/tmp/destination".to_string(),
        read_only: true,
    };
    let scratch = PathBuf::from("/tmp/scratch");
    let _request_path = build_cell_request_path(&scratch);
    let _portable_request = BuildCellSpec {
        root: HostPath::from(PathBuf::from("/tmp/rootfs")),
        scratch: HostPath::from(scratch),
        workspace_path: CellPath::from("/workspace"),
        workspace_seed: WorkspaceSeed::Empty,
        imports: vec![ImportSpec {
            source: HostPath::from(PathBuf::from("/tmp/import")),
            destination: CellPath::from("/workspace/import"),
        }],
        caches: vec![CacheSpec {
            source: HostPath::from(PathBuf::from("/tmp/cache")),
            destination: CellPath::from("/cache"),
        }],
        exports: vec![ExportSpec {
            source: CellPath::from("/workspace/out.txt"),
            destination: HostPath::from(PathBuf::from("/tmp/out.txt")),
        }],
        command: CommandSpec {
            cwd: CellPath::from("/workspace"),
            executable: "/bin/true".to_string(),
            argv: Vec::new(),
        },
        env: Vec::new(),
        network: NetworkPolicy::Enabled,
        limits: ResourceLimits::default(),
        cleanup: CleanupPolicy::Always,
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
    assert_build_cell_reexec_command_signature(build_cell_reexec_command);
    assert_prepare_runtime_emulator_signature(prepare_runtime_emulator);
    assert_prepare_oci_rootfs_signature(prepare_oci_rootfs);
    assert_run_build_cell_signature(run_build_cell);
    assert_finalize_build_cell_signature(finalize_build_cell);
    assert_run_isolated_container_command_signature(run_isolated_container_command);

    let _ = request;
}

#[test]
#[cfg(not(target_os = "linux"))]
fn portable_public_api_is_exported() {
    let _line = SignificantLine {
        number: 1,
        text: "RUN true",
    };
    let _backend_caps = BackendCaps {
        oci_rootfs: false,
        live_bind_mounts: false,
        foreign_arch_exec: false,
        per_job_network_toggle: false,
        profile_selected_network: false,
    };
    let scratch = PathBuf::from("/tmp/scratch");
    let _request_path = build_cell_request_path(&scratch);
    let _portable_request = BuildCellSpec {
        root: HostPath::from(PathBuf::from("/tmp/rootfs")),
        scratch: HostPath::from(scratch),
        workspace_path: CellPath::from("/workspace"),
        workspace_seed: WorkspaceSeed::Empty,
        imports: vec![ImportSpec {
            source: HostPath::from(PathBuf::from("/tmp/import")),
            destination: CellPath::from("/workspace/import"),
        }],
        caches: vec![CacheSpec {
            source: HostPath::from(PathBuf::from("/tmp/cache")),
            destination: CellPath::from("/cache"),
        }],
        exports: vec![ExportSpec {
            source: CellPath::from("/workspace/out.txt"),
            destination: HostPath::from(PathBuf::from("/tmp/out.txt")),
        }],
        command: CommandSpec {
            cwd: CellPath::from("/workspace"),
            executable: "worker".to_string(),
            argv: Vec::new(),
        },
        env: Vec::new(),
        network: NetworkPolicy::Enabled,
        limits: ResourceLimits::default(),
        cleanup: CleanupPolicy::Always,
    };

    assert!(valid_identifier("TARGET"));
    assert_eq!(
        significant_lines("RUN true").next(),
        Some(SignificantLine {
            number: 1,
            text: "RUN true",
        })
    );
}

#[test]
#[cfg(target_os = "linux")]
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
