#![cfg(target_os = "windows")]

// Copyright 2026 Victor Stewart
// SPDX-License-Identifier: Apache-2.0

use metalor::runtime::linux_provider::ProviderShell;
use metalor::runtime::windows::{
    appcontainer_profile_name, build_worker_command, build_worker_process_command,
    copy_worker_exports, parse_wsl_list_output, prepare_job, prepare_worker_request,
    resolve_wsl_distro, sync_worker_caches, validate_application_id, validate_worker_target,
    WorkerTarget, WslProvider, DEFAULT_WSL_DISTRO, WORKER_REQUEST_ENV,
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
        "metalor-windows-support-{label}-{}-{}",
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
fn windows_worker_target_and_profile_name_are_derived() {
    validate_application_id("com.example.metalorexample").unwrap();
    assert_eq!(
        appcontainer_profile_name("com.example.metalorexample"),
        "com.example.metalorexample"
    );

    let target = WorkerTarget::new(
        PathBuf::from(r"C:\metalor\worker.exe"),
        Some("worker-subcommand".to_string()),
    )
    .unwrap();
    validate_worker_target(&target).unwrap();

    let request_path = PathBuf::from(r"C:\metalor\request.json");
    let command = build_worker_command(&target, &request_path).unwrap();
    assert_eq!(
        command.get_program().to_string_lossy(),
        r"C:\metalor\worker.exe"
    );
    assert_eq!(
        command
            .get_envs()
            .find(|(key, _)| key == &std::ffi::OsStr::new(WORKER_REQUEST_ENV))
            .and_then(|(_, value)| value.map(|value| value.to_string_lossy().into_owned())),
        Some(request_path.display().to_string())
    );
}

#[test]
fn wsl_provider_parses_utf16_output_and_builds_shell_commands() {
    let utf16le = [
        0xff, 0xfe, b'U', 0, b'b', 0, b'u', 0, b'n', 0, b't', 0, b'u', 0, b'-', 0, b'2', 0, b'4',
        0, b'.', 0, b'0', 0, b'4', 0, b'\r', 0, b'\n', 0, b'D', 0, b'e', 0, b'b', 0, b'i', 0, b'a',
        0, b'n', 0, b'\r', 0, b'\n', 0,
    ];
    assert_eq!(
        parse_wsl_list_output(&utf16le).unwrap(),
        vec!["Ubuntu-24.04".to_string(), "Debian".to_string()]
    );

    let provider = WslProvider::new(DEFAULT_WSL_DISTRO).unwrap();
    let command = provider.spawn_shell("printf ready").unwrap();
    let args = command
        .get_args()
        .map(|value| value.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    assert_eq!(command.get_program().to_string_lossy(), "wsl.exe");
    assert_eq!(
        args,
        vec![
            "--distribution",
            DEFAULT_WSL_DISTRO,
            "--user",
            "root",
            "--",
            "bash",
            "-lc",
            "printf ready",
        ]
    );
}

#[test]
fn wsl_provider_rejects_empty_distro_names() {
    let error = WslProvider::new("   ").unwrap_err();
    assert!(error.to_string().contains("must not be empty"), "{error:#}");
}

#[test]
fn resolve_wsl_distro_preserves_explicit_nonempty_values() {
    let resolution = resolve_wsl_distro(Some("Debian")).unwrap();
    assert_eq!(resolution.distro, "Debian");
    assert!(!resolution.auto_install);
}

#[test]
fn windows_worker_support_stages_and_exports_build_outputs() {
    let scratch = unique_temp_dir("scratch");
    let job_root = unique_temp_dir("job-root");
    let seed_dir = unique_temp_dir("seed");
    let import_root = unique_temp_dir("import-root");
    let cache_dir = unique_temp_dir("cache");
    let export_root = unique_temp_dir("export-root");
    let import_file = import_root.join("imported.txt");
    let export_file = export_root.join("out.txt");

    fs::write(seed_dir.join("seed.txt"), "seed\r\n").unwrap();
    fs::write(&import_file, "imported\r\n").unwrap();
    fs::write(cache_dir.join("state"), "cached\r\n").unwrap();

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
            executable: "pwsh.exe".to_string(),
            argv: vec![
                "-NoLogo".to_string(),
                "-NoProfile".to_string(),
                "-Command".to_string(),
                concat!(
                    "$content = (Get-Content seed.txt -Raw) + ",
                    "(Get-Content imported.txt -Raw) + ",
                    "(Get-Content ../cache/state -Raw); ",
                    "Set-Content out.txt -Value $content -NoNewline -Encoding utf8NoBOM; ",
                    "Set-Content ../cache/state -Value 'updated' -NoNewline -Encoding utf8NoBOM"
                )
                .to_string(),
            ],
        },
        env: Vec::new(),
        network: NetworkPolicy::Enabled,
        limits: Default::default(),
        cleanup: CleanupPolicy::Always,
    };

    let request_path = prepare_worker_request(&scratch, &spec).unwrap();
    assert!(request_path.is_file());

    let job = prepare_job(&job_root, &spec).unwrap();
    let status = build_worker_process_command(&spec, &job)
        .unwrap()
        .status()
        .unwrap();
    assert!(
        status.success(),
        "Windows worker command failed with {status}"
    );

    sync_worker_caches(&spec, &job).unwrap();
    copy_worker_exports(&spec, &job, true).unwrap();

    assert_eq!(
        fs::read_to_string(export_file).unwrap(),
        "seed\r\nimported\r\ncached\r\n"
    );
    assert_eq!(
        fs::read_to_string(cache_dir.join("state")).unwrap(),
        "updated"
    );
}
