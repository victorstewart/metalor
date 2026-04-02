#![cfg(target_os = "macos")]

// Copyright 2026 Victor Stewart
// SPDX-License-Identifier: Apache-2.0

use metalor::runtime::macos::{
    build_worker_command, copy_worker_exports, helper_environment, prepare_helper_request,
    prepare_job, sync_worker_caches, validate_helper_target, HelperTarget,
    HELPER_INFO_PLIST_TEMPLATE, HELPER_REQUEST_ENV, NETWORKED_HELPER_ENTITLEMENTS_TEMPLATE,
    OFFLINE_HELPER_ENTITLEMENTS_TEMPLATE,
};
use metalor::{
    BuildCellSpec, CacheSpec, CleanupPolicy, CommandSpec, ExportSpec, HostPath, ImportSpec,
    NetworkPolicy, WorkspaceSeed,
};
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

fn unique_temp_dir(label: &str) -> PathBuf {
    let unique = format!(
        "metalor-macos-support-{label}-{}-{}",
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

#[test]
fn helper_target_and_templates_are_wired_for_downstream_apps() {
    let target = HelperTarget::new(
        "com.example.metalorexample",
        "com.example.metalorexample.worker",
    )
    .unwrap();
    validate_helper_target(&target).unwrap();

    assert_eq!(
        target.bundle_relative_path(),
        PathBuf::from("Contents")
            .join("XPCServices")
            .join("com.example.metalorexample.worker.xpc")
    );
    assert_eq!(
        target.executable_relative_path(),
        PathBuf::from("Contents")
            .join("XPCServices")
            .join("com.example.metalorexample.worker.xpc")
            .join("Contents")
            .join("MacOS")
            .join("worker")
    );
    assert!(HELPER_INFO_PLIST_TEMPLATE.contains("CFBundleIdentifier"));
    assert!(NETWORKED_HELPER_ENTITLEMENTS_TEMPLATE.contains("network.client"));
    assert!(OFFLINE_HELPER_ENTITLEMENTS_TEMPLATE.contains("app-sandbox"));
}

#[test]
fn macos_worker_support_stages_and_exports_build_outputs() {
    let scratch = unique_temp_dir("scratch");
    let job_root = unique_temp_dir("job-root");
    let seed_dir = unique_temp_dir("seed");
    let import_root = unique_temp_dir("import-root");
    let cache_dir = unique_temp_dir("cache");
    let export_root = unique_temp_dir("export-root");
    let import_file = import_root.join("imported.txt");
    let export_file = export_root.join("out.txt");

    fs::write(seed_dir.join("seed.txt"), "seed\n").unwrap();
    fs::write(&import_file, "imported\n").unwrap();
    fs::write(cache_dir.join("state"), "cached\n").unwrap();

    let spec = BuildCellSpec {
        root: HostPath::from(unique_temp_dir("root")),
        scratch: HostPath::from(scratch.clone()),
        workspace_path: "/workspace".into(),
        workspace_seed: WorkspaceSeed::SnapshotDir(HostPath::from(seed_dir)),
        imports: vec![ImportSpec {
            source: HostPath::from(import_file),
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
                    "cat seed.txt imported.txt ../cache/state > out.txt\n",
                    "printf updated > ../cache/state\n",
                )
                .to_string(),
            ],
        },
        env: vec![("PATH".to_string(), "/usr/bin:/bin".to_string())],
        network: NetworkPolicy::Enabled,
        limits: Default::default(),
        cleanup: CleanupPolicy::Always,
    };

    let request_path = prepare_helper_request(&scratch, &spec).unwrap();
    let helper_env = helper_environment(&request_path);
    assert_eq!(helper_env[0].0, HELPER_REQUEST_ENV);
    assert_eq!(helper_env[0].1, request_path.display().to_string());

    let job = prepare_job(&job_root, &spec).unwrap();
    let status = build_worker_command(&spec, &job).unwrap().status().unwrap();
    assert!(
        status.success(),
        "macOS worker command failed with {status}"
    );

    sync_worker_caches(&spec, &job).unwrap();
    copy_worker_exports(&spec, &job, true).unwrap();

    assert_eq!(
        fs::read_to_string(export_file).unwrap(),
        "seed\nimported\ncached\n"
    );
    assert_eq!(
        fs::read_to_string(cache_dir.join("state")).unwrap(),
        "updated"
    );
}
