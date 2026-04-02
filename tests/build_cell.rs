#![cfg(target_os = "linux")]

// Copyright 2026 Victor Stewart
// SPDX-License-Identifier: Apache-2.0

use metalor::{
    build_cell_reexec_command, build_cell_request_path, finalize_build_cell, prepare_oci_rootfs,
    read_build_cell_request, run_build_cell, BuildCellSpec, CacheSpec, CleanupPolicy, CommandSpec,
    ExportSpec, HostPath, ImportSpec, NetworkPolicy, WorkspaceSeed,
};
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

const CHILD_MODE_ENV: &str = "METALOR_BUILD_CELL_TEST_CHILD_MODE";
const REMOTE_UBUNTU_REFERENCE: &str =
    "docker.io/library/ubuntu@sha256:186072bba1b2f436cbb91ef2567abca677337cfc786c86e107d25b7072feef0c";

fn unique_temp_dir(label: &str) -> PathBuf {
    let unique = format!(
        "metalor-build-cell-{label}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    );
    let dir = std::env::temp_dir().join(unique);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn spawn_private_runtime_test(
    test_name: &str,
    extra_env: &[(&str, &str)],
) -> std::process::ExitStatus {
    let current_exe = std::env::current_exe().unwrap();
    let mut command = Command::new("unshare");
    command.args(["--fork", "--pid", "--mount", "--uts", "--ipc", "--"]);
    command.arg(&current_exe);
    command.args(["--exact", test_name, "--nocapture"]);
    for (key, value) in extra_env {
        command.env(key, value);
    }
    command.status().unwrap()
}

fn portable_spec(root: &Path, scratch: &Path) -> BuildCellSpec {
    BuildCellSpec {
        root: HostPath::from(root.to_path_buf()),
        scratch: HostPath::from(scratch.to_path_buf()),
        workspace_path: "/workspace".into(),
        workspace_seed: WorkspaceSeed::Empty,
        imports: Vec::new(),
        caches: Vec::new(),
        exports: Vec::new(),
        command: CommandSpec {
            cwd: "/workspace".into(),
            executable: "/bin/true".to_string(),
            argv: Vec::new(),
        },
        env: Vec::new(),
        network: NetworkPolicy::Enabled,
        limits: Default::default(),
        cleanup: CleanupPolicy::Always,
    }
}

#[test]
fn build_cell_reexec_command_writes_request_and_defaults_network_on() {
    let runtime_prefix = unique_temp_dir("reexec-default-network");
    let root = runtime_prefix.join("root");
    let scratch = runtime_prefix.join("scratch");
    let spec = portable_spec(&root, &scratch);

    let command = build_cell_reexec_command(
        Path::new("/usr/bin/tool"),
        "portable-run",
        &runtime_prefix,
        &spec,
    )
    .unwrap();
    let args: Vec<_> = command
        .get_args()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    let request_path = build_cell_request_path(&scratch);

    assert_eq!(command.get_program(), OsStr::new("unshare"));
    assert!(args.contains(&"--build-cell-request".to_string()));
    assert!(args.contains(&request_path.display().to_string()));
    assert!(!args.contains(&"--net".to_string()));
    assert!(
        request_path.is_file(),
        "missing request file {}",
        request_path.display()
    );
}

#[test]
fn build_cell_reexec_command_adds_private_network_namespace_when_disabled() {
    let runtime_prefix = unique_temp_dir("reexec-disabled-network");
    let root = runtime_prefix.join("root");
    let scratch = runtime_prefix.join("scratch");
    let mut spec = portable_spec(&root, &scratch);
    spec.network = NetworkPolicy::Disabled;

    let command = build_cell_reexec_command(
        Path::new("/usr/bin/tool"),
        "portable-run",
        &runtime_prefix,
        &spec,
    )
    .unwrap();
    let args: Vec<_> = command
        .get_args()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();

    assert!(args.contains(&"--net".to_string()));
}

#[test]
fn portable_build_cell_stages_imports_syncs_caches_and_exports() {
    if std::env::var_os(CHILD_MODE_ENV).as_deref() == Some(OsStr::new("portable-run")) {
        let request_path = PathBuf::from(std::env::var("METALOR_BUILD_CELL_REQUEST").unwrap());
        let spec = read_build_cell_request(&request_path).unwrap();
        run_build_cell(&spec).unwrap();
        return;
    }

    let runtime_prefix = unique_temp_dir("portable-runtime-prefix");
    let package_root = runtime_prefix.join("pkg");
    let root = prepare_oci_rootfs(
        REMOTE_UBUNTU_REFERENCE,
        &runtime_prefix,
        &package_root,
        None,
        None,
    )
    .unwrap();
    let scratch = runtime_prefix.join("scratch");
    let seed_dir = unique_temp_dir("portable-seed");
    let import_file = unique_temp_dir("portable-import").join("imported.txt");
    let cache_dir = unique_temp_dir("portable-cache");
    let export_file = unique_temp_dir("portable-export").join("out.txt");
    fs::write(seed_dir.join("seed.txt"), "seed\n").unwrap();
    fs::create_dir_all(import_file.parent().unwrap()).unwrap();
    fs::write(&import_file, "imported\n").unwrap();
    fs::write(cache_dir.join("state"), "cached\n").unwrap();

    let spec = BuildCellSpec {
        root: HostPath::from(root.clone()),
        scratch: HostPath::from(scratch.clone()),
        workspace_path: "/workspace".into(),
        workspace_seed: WorkspaceSeed::SnapshotDir(HostPath::from(seed_dir.clone())),
        imports: vec![ImportSpec {
            source: HostPath::from(import_file.clone()),
            destination: "/workspace/imported.txt".into(),
        }],
        caches: vec![CacheSpec {
            source: HostPath::from(cache_dir.clone()),
            destination: "/cache".into(),
        }],
        exports: vec![ExportSpec {
            source: "/workspace/out.txt".into(),
            destination: HostPath::from(export_file.clone()),
        }],
        command: CommandSpec {
            cwd: "/workspace".into(),
            executable: "/bin/sh".to_string(),
            argv: vec![
                "-lc".to_string(),
                concat!(
                    "cat seed.txt imported.txt /cache/state > out.txt\n",
                    "printf updated > /cache/state\n",
                )
                .to_string(),
            ],
        },
        env: vec![("PATH".to_string(), "/usr/bin:/bin".to_string())],
        network: NetworkPolicy::Enabled,
        limits: Default::default(),
        cleanup: CleanupPolicy::Always,
    };

    build_cell_reexec_command(
        Path::new("/usr/bin/tool"),
        "portable-run",
        &runtime_prefix,
        &spec,
    )
    .unwrap();
    let request_path = build_cell_request_path(&scratch);

    let status = spawn_private_runtime_test(
        "portable_build_cell_stages_imports_syncs_caches_and_exports",
        &[
            (CHILD_MODE_ENV, "portable-run"),
            ("METALOR_PRIVATE_NS", "1"),
            (
                "METALOR_RUNTIME_ROOT_PREFIX",
                runtime_prefix.to_str().unwrap(),
            ),
            ("METALOR_BUILD_CELL_REQUEST", request_path.to_str().unwrap()),
        ],
    );
    assert!(status.success(), "child test failed with {status}");

    let result = finalize_build_cell(&spec, true).unwrap();
    assert!(!result.scratch_preserved);
    assert_eq!(
        fs::read_to_string(&export_file).unwrap(),
        "seed\nimported\ncached\n"
    );
    assert_eq!(
        fs::read_to_string(cache_dir.join("state")).unwrap(),
        "updated"
    );
    assert!(!scratch.exists(), "scratch should be removed after success");
}
