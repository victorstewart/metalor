// Copyright 2026 Victor Stewart
// SPDX-License-Identifier: Apache-2.0

use metalor::runtime::linux_provider::{
    parse_provider_metadata_env, provider_metadata_path, read_provider_metadata_env,
    render_provider_metadata_env, write_provider_metadata_env, LocalLinuxProviderKind,
    LocalLinuxProviderSelection, ProviderRuntimeLayout, ProviderRuntimeMetadata, ProviderSession,
    WarmState, PROVIDER_METADATA_FILE, PROVIDER_RUNTIME_LAYOUT_VERSION,
};
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::str::FromStr;
use std::time::{SystemTime, UNIX_EPOCH};

fn unique_temp_dir(label: &str) -> PathBuf {
    let unique = format!(
        "metalor-provider-{label}-{}-{}",
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

#[cfg(not(target_os = "windows"))]
fn local_provider_shell(script: &str) -> anyhow::Result<Command> {
    let mut command = Command::new("sh");
    command.args(["-lc", script]);
    Ok(command)
}

#[test]
fn parses_local_linux_provider_selection_values() {
    assert_eq!(
        LocalLinuxProviderSelection::from_str("auto").unwrap(),
        LocalLinuxProviderSelection::Auto
    );
    assert_eq!(
        LocalLinuxProviderSelection::from_str("wsl2").unwrap(),
        LocalLinuxProviderSelection::Wsl2
    );
    assert_eq!(
        LocalLinuxProviderSelection::from_str("mac-local").unwrap(),
        LocalLinuxProviderSelection::MacLocal
    );
    assert_eq!(
        LocalLinuxProviderSelection::from_str("").unwrap(),
        LocalLinuxProviderSelection::Auto
    );
    let error = LocalLinuxProviderSelection::from_str("bogus").unwrap_err();
    assert!(
        error
            .to_string()
            .contains("expected one of: auto, wsl2, mac-local"),
        "{error:#}"
    );
}

#[test]
fn renders_and_parses_provider_runtime_metadata() {
    let metadata = ProviderRuntimeMetadata {
        kind: LocalLinuxProviderKind::Wsl2,
        identity: "Ubuntu-24.04".to_string(),
        runtime_root: "/var/tmp/depos-provider/v1/example".to_string(),
        runtime_layout_version: "v1".to_string(),
        bootstrap_version: "v1".to_string(),
        bootstrap_stamp: "/var/tmp/depos-provider/v1/example/bootstrap-v1.stamp".to_string(),
    };
    let rendered = render_provider_metadata_env(&metadata).unwrap();
    assert!(rendered.contains("provider_kind=wsl2"));
    assert!(rendered.contains("provider_identity=Ubuntu-24.04"));

    let parsed = parse_provider_metadata_env(&rendered).unwrap();
    assert_eq!(parsed, metadata);
}

#[test]
fn writes_and_reads_provider_runtime_metadata_file() {
    let root = unique_temp_dir("metadata");
    let path = root.join(PROVIDER_METADATA_FILE);
    let metadata = ProviderRuntimeMetadata {
        kind: LocalLinuxProviderKind::MacLocal,
        identity: "depos".to_string(),
        runtime_root: "/var/tmp/depos-provider/v1/example".to_string(),
        runtime_layout_version: "v1".to_string(),
        bootstrap_version: "v2".to_string(),
        bootstrap_stamp: "/var/tmp/depos-provider/v1/example/bootstrap-v2.stamp".to_string(),
    };

    write_provider_metadata_env(&path, &metadata).unwrap();
    let read_back = read_provider_metadata_env(&path).unwrap();
    assert_eq!(read_back, metadata);
}

#[test]
fn provider_metadata_path_appends_the_default_file_name() {
    assert_eq!(
        provider_metadata_path("/var/tmp/depos-provider/v1/example"),
        "/var/tmp/depos-provider/v1/example/provider-metadata.env"
    );
    assert_eq!(provider_metadata_path("/"), "/provider-metadata.env");
}

#[test]
fn provider_runtime_layout_builds_paths_and_metadata() {
    let layout = ProviderRuntimeLayout::new("/var/tmp/metalor-provider/v1/example/").unwrap();
    assert_eq!(layout.root(), "/var/tmp/metalor-provider/v1/example");
    assert_eq!(
        layout.jobs_root(),
        "/var/tmp/metalor-provider/v1/example/jobs"
    );
    assert_eq!(
        layout.metadata_path(),
        "/var/tmp/metalor-provider/v1/example/provider-metadata.env"
    );
    assert_eq!(
        layout.join("repo-source/metalor").unwrap(),
        "/var/tmp/metalor-provider/v1/example/repo-source/metalor"
    );
    assert_eq!(
        layout.stamp_path("bootstrap", "v1").unwrap(),
        "/var/tmp/metalor-provider/v1/example/bootstrap-v1.stamp"
    );
    let metadata = layout
        .metadata(LocalLinuxProviderKind::Wsl2, "Ubuntu-24.04", "v1")
        .unwrap();
    assert_eq!(metadata.runtime_root, layout.root());
    assert_eq!(
        metadata.runtime_layout_version,
        PROVIDER_RUNTIME_LAYOUT_VERSION
    );
    assert_eq!(
        metadata.bootstrap_stamp,
        "/var/tmp/metalor-provider/v1/example/bootstrap-v1.stamp"
    );
}

#[cfg(not(target_os = "windows"))]
#[test]
fn shell_helpers_run_commands_and_probe_provider_paths() {
    let runtime_root = unique_temp_dir("shell");
    let layout = ProviderRuntimeLayout::new(runtime_root.display().to_string()).unwrap();
    let session = ProviderSession::new(local_provider_shell);
    let mut log = String::new();
    let job = session
        .prepare_job_root(&layout, "shell", &mut log)
        .unwrap();
    let marker = PathBuf::from(&job.root).join("marker.txt");
    session
        .run(
            &format!("printf ready > {}", provider_metadata_path(&job.root)),
            &mut log,
        )
        .unwrap();
    assert!(session
        .path_exists(&provider_metadata_path(&job.root))
        .unwrap());
    fs::write(&marker, "ok").unwrap();
    assert!(session.path_exists(&marker.display().to_string()).unwrap());
    session.remove_path(&job.root, &mut log).unwrap();
    assert!(!session.path_exists(&job.root).unwrap());
}

#[cfg(not(target_os = "windows"))]
#[test]
fn session_writes_provider_runtime_metadata() {
    let runtime_root = unique_temp_dir("session-metadata");
    let layout = ProviderRuntimeLayout::new(runtime_root.display().to_string()).unwrap();
    let metadata = layout
        .metadata(LocalLinuxProviderKind::MacLocal, "metalor", "v2")
        .unwrap();
    let session = ProviderSession::new(local_provider_shell);
    let mut log = String::new();
    session.write_runtime_metadata(&metadata, &mut log).unwrap();
    let written = read_provider_metadata_env(runtime_root.join(PROVIDER_METADATA_FILE)).unwrap();
    assert_eq!(written, metadata);
}

#[cfg(not(target_os = "windows"))]
#[test]
fn session_ensures_warm_state_only_runs_cold_setup_once() {
    let runtime_root = unique_temp_dir("warm-state");
    let layout = ProviderRuntimeLayout::new(runtime_root.display().to_string()).unwrap();
    let stamp_path = layout.stamp_path("repo-sync", "abc123").unwrap();
    let marker_path = layout.join("repo-source/marker.txt").unwrap();
    let session = ProviderSession::new(local_provider_shell);
    let mut log = String::new();
    let first = session
        .ensure_warm_state(
            "source sync",
            &stamp_path,
            &[&marker_path],
            &mut log,
            |session, log| {
                session.run(
                    &format!(
                        "mkdir -p {} && printf ready > {}",
                        PathBuf::from(&marker_path).parent().unwrap().display(),
                        marker_path
                    ),
                    log,
                )
            },
        )
        .unwrap();
    assert_eq!(first, WarmState::Cold);
    assert!(PathBuf::from(&stamp_path).is_file());
    assert!(PathBuf::from(&marker_path).is_file());

    let second = session
        .ensure_warm_state(
            "source sync",
            &stamp_path,
            &[&marker_path],
            &mut log,
            |_session, _log| panic!("cold setup should not run on warm path"),
        )
        .unwrap();
    assert_eq!(second, WarmState::Warm);
    assert!(log.contains("provider source sync: cold"));
    assert!(log.contains("provider source sync: warm"));
}

#[cfg(not(target_os = "windows"))]
#[test]
fn tar_sync_helpers_push_pull_and_restore_bin_permissions() {
    use std::os::unix::fs::PermissionsExt;

    let local_root = unique_temp_dir("local-push");
    let local_package = local_root.join("package");
    let local_bin = local_package.join("bin");
    let local_include = local_package.join("include");
    let remote_parent = unique_temp_dir("remote-parent");
    let pulled_parent = unique_temp_dir("pulled-parent");
    fs::create_dir_all(&local_bin).unwrap();
    fs::create_dir_all(&local_include).unwrap();
    let local_tool = local_bin.join("tool");
    fs::write(&local_tool, "#!/bin/sh\necho hi\n").unwrap();
    fs::set_permissions(&local_tool, fs::Permissions::from_mode(0o644)).unwrap();
    fs::write(local_include.join("demo.h"), "#pragma once\n").unwrap();

    let session = ProviderSession::new(local_provider_shell);
    let mut log = String::new();
    let remote_root = session
        .stage_host_path(
            &local_package,
            &remote_parent.display().to_string(),
            true,
            &mut log,
        )
        .unwrap();
    assert!(log.contains("push into provider with tar"));

    let remote_tool = PathBuf::from(&remote_root).join("bin").join("tool");
    let mode = fs::metadata(&remote_tool).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o755);

    let remote_header = PathBuf::from(&remote_root).join("include").join("demo.h");
    session
        .collect_path(
            &remote_header.display().to_string(),
            &pulled_parent,
            &mut log,
        )
        .unwrap();
    assert_eq!(
        fs::read_to_string(pulled_parent.join("demo.h")).unwrap(),
        "#pragma once\n"
    );
}

#[cfg(not(target_os = "windows"))]
#[test]
fn collect_path_reports_child_failure_instead_of_pipe_error() {
    let session = ProviderSession::new(|_script: &str| -> anyhow::Result<Command> {
        let mut command = Command::new("sh");
        command.args(["-lc", "yes x | head -c 1048576"]);
        Ok(command)
    });
    let mut log = String::new();
    let pulled_parent = unique_temp_dir("broken-pipe");
    let error = session
        .collect_path("/tmp/provider/fake.tar", &pulled_parent, &mut log)
        .unwrap_err();
    let message = format!("{error:#}");
    assert!(
        !message.contains("failed to pipe provider tar into host tar"),
        "{message}"
    );
    assert!(
        message.contains("host tar failed") || message.contains("provider tar failed"),
        "{message}"
    );
}
