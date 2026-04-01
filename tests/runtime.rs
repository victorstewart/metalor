// Copyright 2026 Victor Stewart
// SPDX-License-Identifier: Apache-2.0

use metalor::runtime::{
    build_unshare_reexec_command, helper_binary_path, prepare_oci_rootfs, prepare_runtime_emulator,
    run_isolated_container_command, BindMount, ContainerRunCommand, RUN_HELPER_DIR,
};
use std::ffi::OsStr;
use std::fs;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

const CHILD_MODE_ENV: &str = "METALOR_RUNTIME_TEST_CHILD_MODE";
const REMOTE_UBUNTU_REFERENCE: &str =
    "docker.io/library/ubuntu@sha256:186072bba1b2f436cbb91ef2567abca677337cfc786c86e107d25b7072feef0c";

fn unique_temp_dir(label: &str) -> std::path::PathBuf {
    let unique = format!(
        "metalor-{label}-{}-{}",
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

fn run_host_command(command: &mut Command) {
    let status = command.status().unwrap();
    assert!(status.success(), "{command:?} failed with {status}");
}

fn write_runtime_root_sentinel(root: &Path, runtime_prefix: &Path) {
    fs::write(
        root.join(".metalor-root"),
        runtime_prefix
            .canonicalize()
            .unwrap()
            .as_os_str()
            .as_encoded_bytes(),
    )
    .unwrap();
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

fn host_arch() -> &'static str {
    match std::env::consts::ARCH {
        "x86_64" | "amd64" => "x86_64",
        "aarch64" | "arm64" => "aarch64",
        "riscv64" => "riscv64",
        other => panic!("unsupported host arch {other}"),
    }
}

fn foreign_arch() -> &'static str {
    match host_arch() {
        "x86_64" => "aarch64",
        "aarch64" => "x86_64",
        "riscv64" => "x86_64",
        other => panic!("unsupported host arch {other}"),
    }
}

fn host_system_mounts() -> Vec<BindMount> {
    [
        ("/bin", "/bin"),
        ("/usr", "/usr"),
        ("/lib", "/lib"),
        ("/lib64", "/lib64"),
    ]
    .into_iter()
    .filter_map(|(source, destination)| {
        let source = Path::new(source);
        if !source.exists() {
            return None;
        }
        Some(BindMount {
            source: fs::canonicalize(source).unwrap_or_else(|_| source.to_path_buf()),
            destination: destination.to_string(),
            read_only: true,
        })
    })
    .collect()
}

fn expected_elf_machine(arch: &str) -> u16 {
    match arch {
        "x86_64" => 62,
        "aarch64" => 183,
        "riscv64" => 243,
        other => panic!("unsupported arch {other}"),
    }
}

fn expected_oci_arch(arch: &str) -> &'static str {
    match arch {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        "riscv64" => "riscv64",
        other => panic!("unsupported arch {other}"),
    }
}

fn elf_machine(binary: &Path) -> u16 {
    let bytes = fs::read(binary).unwrap();
    assert!(
        bytes.len() >= 20,
        "ELF binary {} was unexpectedly short",
        binary.display()
    );
    assert_eq!(
        &bytes[..4],
        b"\x7fELF",
        "{} did not start with ELF magic",
        binary.display()
    );
    u16::from_le_bytes([bytes[18], bytes[19]])
}

#[test]
fn helper_binary_paths_live_under_the_shared_runtime_dir() {
    assert_eq!(RUN_HELPER_DIR, "/.metalor-run");
    assert_eq!(
        helper_binary_path("qemu-aarch64-static"),
        "/.metalor-run/qemu-aarch64-static"
    );
}

#[test]
fn builds_unshare_reexec_command_for_the_hidden_runner() {
    let runtime_prefix = unique_temp_dir("prefix");
    let runtime_root = runtime_prefix.join("root");
    let request = ContainerRunCommand {
        root: runtime_root.clone(),
        cwd: "/work".to_string(),
        mounts: vec![BindMount {
            source: Path::new("/host/usr").to_path_buf(),
            destination: "/usr".to_string(),
            read_only: true,
        }],
        env: vec![("PATH".to_string(), "/usr/bin:/bin".to_string())],
        emulator: Some("/.metalor-run/qemu-aarch64-static".to_string()),
        executable: "/bin/sh".to_string(),
        argv: vec!["-lc".to_string(), "echo hi".to_string()],
    };

    let command = build_unshare_reexec_command(
        Path::new("/usr/bin/tool"),
        "internal-run",
        &runtime_prefix,
        &request,
    )
    .unwrap();
    let args: Vec<_> = command
        .get_args()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    let envs: Vec<_> = command
        .get_envs()
        .map(|(key, value)| {
            (
                key.to_string_lossy().into_owned(),
                value.map(|value| value.to_string_lossy().into_owned()),
            )
        })
        .collect();

    assert_eq!(command.get_program(), OsStr::new("unshare"));
    let expected_args = vec![
        "--fork".to_string(),
        "--pid".to_string(),
        "--mount".to_string(),
        "--uts".to_string(),
        "--ipc".to_string(),
        "--".to_string(),
        "/usr/bin/tool".to_string(),
        "internal-run".to_string(),
        "--root".to_string(),
        runtime_root.canonicalize().unwrap().display().to_string(),
        "--cwd".to_string(),
        "/work".to_string(),
        "--mount-source".to_string(),
        "/host/usr".to_string(),
        "--mount-dest".to_string(),
        "/usr".to_string(),
        "--mount-mode".to_string(),
        "ro".to_string(),
        "--env".to_string(),
        "PATH=/usr/bin:/bin".to_string(),
        "--emulator".to_string(),
        "/.metalor-run/qemu-aarch64-static".to_string(),
        "--executable".to_string(),
        "/bin/sh".to_string(),
        "-lc".to_string(),
        "echo hi".to_string(),
    ];
    assert_eq!(args, expected_args);
    assert!(
        runtime_root.join(".metalor-root").is_file(),
        "missing runtime root sentinel"
    );
    assert!(envs.contains(&("METALOR_PRIVATE_NS".to_string(), Some("1".to_string()))));
    assert!(envs.contains(&(
        "METALOR_RUNTIME_ROOT_PREFIX".to_string(),
        Some(runtime_prefix.canonicalize().unwrap().display().to_string())
    )));
}

#[test]
fn rejects_container_environment_keys_with_equals_signs() {
    let runtime_prefix = unique_temp_dir("bad-env-key");
    let runtime_root = runtime_prefix.join("root");
    let request = ContainerRunCommand {
        root: runtime_root,
        cwd: "/".to_string(),
        mounts: Vec::new(),
        env: vec![("BAD=KEY".to_string(), "value".to_string())],
        emulator: None,
        executable: "/bin/true".to_string(),
        argv: Vec::new(),
    };

    let error = build_unshare_reexec_command(
        Path::new("/usr/bin/tool"),
        "internal-run",
        &runtime_prefix,
        &request,
    )
    .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("container environment keys must not contain '='"),
        "{error:#}"
    );
}

#[test]
fn rejects_empty_or_nul_container_environment_entries() {
    let runtime_prefix = unique_temp_dir("bad-env-entry");
    let cases = [
        (
            vec![("".to_string(), "value".to_string())],
            "container environment keys must not be empty",
        ),
        (
            vec![("BAD\0KEY".to_string(), "value".to_string())],
            "container environment keys must not contain NUL bytes",
        ),
        (
            vec![("GOOD_KEY".to_string(), "bad\0value".to_string())],
            "container environment values must not contain NUL bytes",
        ),
    ];

    for (env, expected) in cases {
        let error = build_unshare_reexec_command(
            Path::new("/usr/bin/tool"),
            "internal-run",
            &runtime_prefix,
            &ContainerRunCommand {
                root: runtime_prefix.join("root"),
                cwd: "/".to_string(),
                mounts: Vec::new(),
                env,
                emulator: None,
                executable: "/bin/true".to_string(),
                argv: Vec::new(),
            },
        )
        .unwrap_err();
        assert!(error.to_string().contains(expected), "{error:#}");
    }
}

#[test]
fn accepts_bare_command_executables_before_reexec() {
    let runtime_prefix = unique_temp_dir("bare-command");
    let command = build_unshare_reexec_command(
        Path::new("/usr/bin/tool"),
        "internal-run",
        &runtime_prefix,
        &ContainerRunCommand {
            root: runtime_prefix.join("root"),
            cwd: "/".to_string(),
            mounts: Vec::new(),
            env: Vec::new(),
            emulator: None,
            executable: "env".to_string(),
            argv: vec!["-0".to_string()],
        },
    )
    .unwrap();
    let args: Vec<_> = command
        .get_args()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    assert!(args.contains(&"--executable".to_string()));
    assert!(args.contains(&"env".to_string()));
}

#[test]
fn rejects_prepare_runtime_emulator_for_unsupported_architectures() {
    let runtime_root = unique_temp_dir("unsupported-emulator-arch");

    let host_error = prepare_runtime_emulator(&runtime_root, "sparc64", host_arch()).unwrap_err();
    assert!(
        host_error
            .to_string()
            .contains("unsupported architecture sparc64"),
        "{host_error:#}"
    );

    let guest_error = prepare_runtime_emulator(&runtime_root, host_arch(), "sparc64").unwrap_err();
    assert!(
        guest_error
            .to_string()
            .contains("unsupported architecture sparc64"),
        "{guest_error:#}"
    );
}

#[test]
fn rejects_prepare_runtime_emulator_when_required_qemu_is_missing_from_path() {
    if std::env::var_os(CHILD_MODE_ENV).as_deref() == Some(OsStr::new("missing-qemu")) {
        let runtime_root = PathBuf::from(std::env::var("METALOR_TEST_RUNTIME_ROOT").unwrap());
        let error =
            prepare_runtime_emulator(&runtime_root, host_arch(), foreign_arch()).unwrap_err();
        let foreign = foreign_arch();
        assert!(
            error.to_string().contains(&format!(
                "foreign-architecture execution for guest {foreign}"
            )),
            "{error:#}"
        );
        assert!(error.to_string().contains("requires qemu-"), "{error:#}");
        assert!(error.to_string().contains("in PATH"), "{error:#}");
        return;
    }

    let current_exe = std::env::current_exe().unwrap();
    let runtime_root = unique_temp_dir("missing-qemu-root");
    let status = Command::new(current_exe)
        .args([
            "--exact",
            "rejects_prepare_runtime_emulator_when_required_qemu_is_missing_from_path",
            "--nocapture",
        ])
        .env(CHILD_MODE_ENV, "missing-qemu")
        .env("METALOR_TEST_RUNTIME_ROOT", runtime_root.as_os_str())
        .env("PATH", "")
        .status()
        .unwrap();
    assert!(status.success(), "child test failed with {status}");
}

#[test]
fn rejects_container_root_outside_runtime_root_prefix() {
    let runtime_prefix = unique_temp_dir("prefix-outside");
    let other_prefix = unique_temp_dir("outside");
    let request = ContainerRunCommand {
        root: other_prefix.join("root"),
        cwd: "/".to_string(),
        mounts: Vec::new(),
        env: Vec::new(),
        emulator: None,
        executable: "/bin/true".to_string(),
        argv: Vec::new(),
    };

    let error = build_unshare_reexec_command(
        Path::new("/usr/bin/tool"),
        "internal-run",
        &runtime_prefix,
        &request,
    )
    .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("must live under runtime root prefix"),
        "{error:#}"
    );
}

#[test]
fn rejects_container_roots_through_symlinked_prefix_children_without_creating_them() {
    let runtime_prefix = unique_temp_dir("symlinked-root-prefix");
    let outside_root = unique_temp_dir("symlinked-root-outside");
    let link_path = runtime_prefix.join("link");
    symlink(&outside_root, &link_path).unwrap();
    let request = ContainerRunCommand {
        root: link_path.join("root"),
        cwd: "/".to_string(),
        mounts: Vec::new(),
        env: Vec::new(),
        emulator: None,
        executable: "/bin/true".to_string(),
        argv: Vec::new(),
    };

    let error = build_unshare_reexec_command(
        Path::new("/usr/bin/tool"),
        "internal-run",
        &runtime_prefix,
        &request,
    )
    .unwrap_err();
    assert!(
        error.to_string().contains("traverses a symlink")
            || error
                .to_string()
                .contains("must live under runtime root prefix"),
        "{error:#}"
    );
    assert!(
        !outside_root.join("root").exists(),
        "unexpected host-side creation outside the runtime prefix"
    );
}

#[test]
fn rejects_runtime_root_sentinel_symlinks_without_writing_through_them() {
    let runtime_prefix = unique_temp_dir("sentinel-symlink-prefix");
    let runtime_root = runtime_prefix.join("root");
    let outside_root = unique_temp_dir("sentinel-symlink-outside");
    let outside_file = outside_root.join("leaked-sentinel");
    fs::create_dir_all(&runtime_root).unwrap();
    symlink(&outside_file, runtime_root.join(".metalor-root")).unwrap();

    let request = ContainerRunCommand {
        root: runtime_root,
        cwd: "/".to_string(),
        mounts: Vec::new(),
        env: Vec::new(),
        emulator: None,
        executable: "/bin/true".to_string(),
        argv: Vec::new(),
    };

    let error = build_unshare_reexec_command(
        Path::new("/usr/bin/tool"),
        "internal-run",
        &runtime_prefix,
        &request,
    )
    .unwrap_err();
    assert!(
        error.to_string().contains("must not be a symlink"),
        "{error:#}"
    );
    assert!(
        !outside_file.exists(),
        "unexpected sentinel write through a symlink"
    );
}

#[test]
fn rejects_running_inside_the_host_mount_namespace() {
    let runtime_prefix = unique_temp_dir("host-ns");
    let runtime_root = runtime_prefix.join("root");
    fs::create_dir_all(&runtime_root).unwrap();
    fs::write(
        runtime_root.join(".metalor-root"),
        runtime_prefix
            .canonicalize()
            .unwrap()
            .as_os_str()
            .as_encoded_bytes(),
    )
    .unwrap();

    std::env::set_var("METALOR_PRIVATE_NS", "1");
    std::env::set_var(
        "METALOR_RUNTIME_ROOT_PREFIX",
        runtime_prefix.canonicalize().unwrap(),
    );
    let request = ContainerRunCommand {
        root: runtime_root,
        cwd: "/".to_string(),
        mounts: Vec::new(),
        env: Vec::new(),
        emulator: None,
        executable: "/bin/true".to_string(),
        argv: Vec::new(),
    };

    let error = run_isolated_container_command(&request).unwrap_err();
    assert!(
        error.to_string().contains("host mount namespace"),
        "{error:#}"
    );
    std::env::remove_var("METALOR_PRIVATE_NS");
    std::env::remove_var("METALOR_RUNTIME_ROOT_PREFIX");
}

#[test]
fn rejects_relative_bind_mount_sources_before_reexec() {
    let runtime_prefix = unique_temp_dir("relative-mount-source");
    let runtime_root = runtime_prefix.join("root");
    let request = ContainerRunCommand {
        root: runtime_root,
        cwd: "/".to_string(),
        mounts: vec![BindMount {
            source: Path::new("relative/usr").to_path_buf(),
            destination: "/usr".to_string(),
            read_only: true,
        }],
        env: Vec::new(),
        emulator: None,
        executable: "/bin/true".to_string(),
        argv: Vec::new(),
    };

    let error = build_unshare_reexec_command(
        Path::new("/usr/bin/tool"),
        "internal-run",
        &runtime_prefix,
        &request,
    )
    .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("bind mount source must be absolute"),
        "{error:#}"
    );
}

#[test]
fn rejects_bind_mount_sources_with_parent_components_before_reexec() {
    let runtime_prefix = unique_temp_dir("parent-mount-source");
    let runtime_root = runtime_prefix.join("root");
    let request = ContainerRunCommand {
        root: runtime_root,
        cwd: "/".to_string(),
        mounts: vec![BindMount {
            source: Path::new("/tmp/../usr").to_path_buf(),
            destination: "/usr".to_string(),
            read_only: true,
        }],
        env: Vec::new(),
        emulator: None,
        executable: "/bin/true".to_string(),
        argv: Vec::new(),
    };

    let error = build_unshare_reexec_command(
        Path::new("/usr/bin/tool"),
        "internal-run",
        &runtime_prefix,
        &request,
    )
    .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("bind mount source must not contain '..'"),
        "{error:#}"
    );
}

#[test]
fn rejects_non_absolute_or_parent_container_paths_before_reexec() {
    let runtime_prefix = unique_temp_dir("bad-container-paths");
    let runtime_root = runtime_prefix.join("root");

    let cwd_error = build_unshare_reexec_command(
        Path::new("/usr/bin/tool"),
        "internal-run",
        &runtime_prefix,
        &ContainerRunCommand {
            root: runtime_root.clone(),
            cwd: "work".to_string(),
            mounts: Vec::new(),
            env: Vec::new(),
            emulator: None,
            executable: "/bin/true".to_string(),
            argv: Vec::new(),
        },
    )
    .unwrap_err();
    assert!(
        cwd_error
            .to_string()
            .contains("container cwd must be absolute"),
        "{cwd_error:#}"
    );

    let parent_error = build_unshare_reexec_command(
        Path::new("/usr/bin/tool"),
        "internal-run",
        &runtime_prefix,
        &ContainerRunCommand {
            root: runtime_root.clone(),
            cwd: "/../work".to_string(),
            mounts: Vec::new(),
            env: Vec::new(),
            emulator: None,
            executable: "/bin/true".to_string(),
            argv: Vec::new(),
        },
    )
    .unwrap_err();
    assert!(
        parent_error
            .to_string()
            .contains("container cwd must not contain '..'"),
        "{parent_error:#}"
    );

    let executable_error = build_unshare_reexec_command(
        Path::new("/usr/bin/tool"),
        "internal-run",
        &runtime_prefix,
        &ContainerRunCommand {
            root: runtime_root,
            cwd: "/".to_string(),
            mounts: Vec::new(),
            env: Vec::new(),
            emulator: None,
            executable: "bin/true".to_string(),
            argv: Vec::new(),
        },
    )
    .unwrap_err();
    assert!(
        executable_error
            .to_string()
            .contains("container executable must be absolute"),
        "{executable_error:#}"
    );
}

#[test]
fn rejects_emulator_paths_outside_the_shared_helper_dir() {
    let runtime_prefix = unique_temp_dir("bad-emulator");
    let runtime_root = runtime_prefix.join("root");
    let request = ContainerRunCommand {
        root: runtime_root,
        cwd: "/".to_string(),
        mounts: Vec::new(),
        env: Vec::new(),
        emulator: Some("/tmp/qemu-aarch64-static".to_string()),
        executable: "/bin/true".to_string(),
        argv: Vec::new(),
    };

    let error = build_unshare_reexec_command(
        Path::new("/usr/bin/tool"),
        "internal-run",
        &runtime_prefix,
        &request,
    )
    .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("container emulator must live under"),
        "{error:#}"
    );
}

#[test]
fn prepares_runtime_emulator_for_foreign_arch_execution() {
    let runtime_root = unique_temp_dir("emulator-root");

    let emulator = prepare_runtime_emulator(&runtime_root, host_arch(), foreign_arch()).unwrap();
    assert_eq!(
        emulator,
        Some(helper_binary_path(match foreign_arch() {
            "x86_64" => "qemu-x86_64-static",
            "aarch64" => "qemu-aarch64-static",
            "riscv64" => "qemu-riscv64-static",
            other => panic!("unsupported foreign arch {other}"),
        }))
    );
    assert!(
        runtime_root
            .join(emulator.unwrap().trim_start_matches('/'))
            .is_file(),
        "missing staged emulator under {}",
        runtime_root.display()
    );
}

#[test]
fn rejects_helper_dir_symlinks_without_copying_helpers_through_them() {
    let runtime_root = unique_temp_dir("emulator-helper-symlink-root");
    let outside_root = unique_temp_dir("emulator-helper-symlink-outside");
    symlink(
        &outside_root,
        runtime_root.join(RUN_HELPER_DIR.trim_start_matches('/')),
    )
    .unwrap();

    let error = prepare_runtime_emulator(&runtime_root, host_arch(), foreign_arch()).unwrap_err();
    assert!(
        error.to_string().contains("traverses a symlink"),
        "{error:#}"
    );
    assert!(
        fs::read_dir(&outside_root).unwrap().next().is_none(),
        "unexpected helper copy through a symlinked helper dir"
    );
}

#[test]
fn prepares_oci_rootfs_inside_the_runtime_prefix() {
    let runtime_prefix = unique_temp_dir("oci-prefix");
    let seed_layout = runtime_prefix.join("seed-layout");
    let package_root = runtime_prefix.join("pkg");

    run_host_command(
        Command::new("umoci")
            .args(["init", "--layout"])
            .arg(&seed_layout),
    );
    run_host_command(
        Command::new("umoci")
            .arg("new")
            .arg("--image")
            .arg(format!("{}:base", seed_layout.display())),
    );

    let rootfs = prepare_oci_rootfs(
        &format!("oci:{}:base", seed_layout.display()),
        &runtime_prefix,
        &package_root,
        None,
        None,
    )
    .unwrap();

    assert!(
        rootfs.is_dir(),
        "missing unpacked rootfs at {}",
        rootfs.display()
    );
    assert!(rootfs.starts_with(package_root.canonicalize().unwrap()));
}

#[test]
fn rejects_invalid_remote_oci_references_with_copy_context() {
    let runtime_prefix = unique_temp_dir("bad-remote-oci-prefix");
    let package_root = runtime_prefix.join("pkg");

    let error = prepare_oci_rootfs(
        "docker://::not-a-reference",
        &runtime_prefix,
        &package_root,
        None,
        None,
    )
    .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("copy OCI image docker://::not-a-reference"),
        "{error:#}"
    );
}

#[test]
fn rejects_unsupported_requested_oci_arch_before_download() {
    let runtime_prefix = unique_temp_dir("bad-oci-arch-prefix");
    let package_root = runtime_prefix.join("pkg");

    let error = prepare_oci_rootfs(
        REMOTE_UBUNTU_REFERENCE,
        &runtime_prefix,
        &package_root,
        None,
        Some("sparc64"),
    )
    .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("unsupported architecture sparc64"),
        "{error:#}"
    );
}

#[test]
fn prepares_remote_oci_rootfs_from_a_pinned_ubuntu_reference() {
    let runtime_prefix = unique_temp_dir("remote-oci-prefix");
    let package_root = runtime_prefix.join("pkg");

    let rootfs = prepare_oci_rootfs(
        REMOTE_UBUNTU_REFERENCE,
        &runtime_prefix,
        &package_root,
        None,
        None,
    )
    .unwrap();

    assert!(
        rootfs.is_dir(),
        "missing unpacked rootfs at {}",
        rootfs.display()
    );
    assert!(rootfs.starts_with(package_root.canonicalize().unwrap()));

    let os_release = fs::read_to_string(rootfs.join("etc/os-release")).unwrap();
    assert!(
        os_release.contains("ID=ubuntu"),
        "unexpected /etc/os-release contents:\n{os_release}"
    );
    assert!(
        os_release.contains("VERSION_ID=\"24.04\""),
        "unexpected /etc/os-release contents:\n{os_release}"
    );
}
#[test]
fn executes_hidden_runner_successfully_and_honors_explicit_auto_mount_overrides() {
    if std::env::var_os(CHILD_MODE_ENV).as_deref() == Some(OsStr::new("happy-path")) {
        let runtime_prefix =
            Path::new(&std::env::var("METALOR_TEST_RUNTIME_PREFIX").unwrap()).to_path_buf();
        let runtime_root = runtime_prefix.join("root");
        let work_dir = PathBuf::from(std::env::var("METALOR_TEST_WORK_DIR").unwrap());
        let custom_resolv = PathBuf::from(std::env::var("METALOR_TEST_CUSTOM_RESOLV").unwrap());
        let custom_null = PathBuf::from(std::env::var("METALOR_TEST_CUSTOM_NULL").unwrap());
        fs::create_dir_all(&runtime_root).unwrap();
        write_runtime_root_sentinel(&runtime_root, &runtime_prefix);

        let mut mounts = host_system_mounts();
        mounts.push(BindMount {
            source: work_dir,
            destination: "/work".to_string(),
            read_only: false,
        });
        mounts.push(BindMount {
            source: custom_resolv,
            destination: "/etc/resolv.conf".to_string(),
            read_only: true,
        });
        mounts.push(BindMount {
            source: custom_null,
            destination: "/dev/null".to_string(),
            read_only: false,
        });

        run_isolated_container_command(&ContainerRunCommand {
            root: runtime_root,
            cwd: "/work".to_string(),
            mounts,
            env: vec![
                ("PATH".to_string(), "/usr/bin:/bin".to_string()),
                ("METALOR_TEST_VALUE".to_string(), "ok".to_string()),
            ],
            emulator: None,
            executable: "/bin/sh".to_string(),
            argv: vec![
                "-lc".to_string(),
                concat!(
                    "pwd > /work/pwd
",
                    "printf %s \"$METALOR_TEST_VALUE\" > /work/env
",
                    "IFS= read -r first_line </etc/resolv.conf
",
                    "printf %s \"$first_line\" > /work/resolv
",
                    "printf overridden >/dev/null
",
                    "printf hidden-runner-ok >/work/proof
",
                )
                .to_string(),
            ],
        })
        .unwrap();
        return;
    }

    let runtime_prefix = unique_temp_dir("happy-path-prefix");
    let work_dir = unique_temp_dir("happy-path-work");
    let custom_dir = unique_temp_dir("happy-path-custom");
    let custom_resolv = custom_dir.join("resolv.conf");
    let custom_null = custom_dir.join("null");
    fs::write(
        &custom_resolv,
        "nameserver 203.0.113.7
",
    )
    .unwrap();
    fs::write(&custom_null, "").unwrap();

    let status = spawn_private_runtime_test(
        "executes_hidden_runner_successfully_and_honors_explicit_auto_mount_overrides",
        &[
            (CHILD_MODE_ENV, "happy-path"),
            (
                "METALOR_RUNTIME_ROOT_PREFIX",
                runtime_prefix.to_str().unwrap(),
            ),
            ("METALOR_PRIVATE_NS", "1"),
            (
                "METALOR_TEST_RUNTIME_PREFIX",
                runtime_prefix.to_str().unwrap(),
            ),
            ("METALOR_TEST_WORK_DIR", work_dir.to_str().unwrap()),
            (
                "METALOR_TEST_CUSTOM_RESOLV",
                custom_resolv.to_str().unwrap(),
            ),
            ("METALOR_TEST_CUSTOM_NULL", custom_null.to_str().unwrap()),
        ],
    );
    assert!(status.success(), "child test failed with {status}");
    assert_eq!(
        fs::read_to_string(work_dir.join("pwd")).unwrap(),
        "/work
"
    );
    assert_eq!(fs::read_to_string(work_dir.join("env")).unwrap(), "ok");
    assert_eq!(
        fs::read_to_string(work_dir.join("resolv")).unwrap(),
        "nameserver 203.0.113.7"
    );
    assert_eq!(
        fs::read_to_string(work_dir.join("proof")).unwrap(),
        "hidden-runner-ok"
    );
    assert_eq!(fs::read_to_string(custom_null).unwrap(), "overridden");
}

#[test]
fn requested_oci_arch_selects_expected_binaries_and_partitions_cache() {
    let runtime_prefix = unique_temp_dir("oci-arch-prefix");
    let cache_root = unique_temp_dir("oci-arch-cache");
    let host_package_root = runtime_prefix.join("host-pkg");
    let foreign_package_root = runtime_prefix.join("foreign-pkg");

    let host_rootfs = prepare_oci_rootfs(
        REMOTE_UBUNTU_REFERENCE,
        &runtime_prefix,
        &host_package_root,
        Some(&cache_root),
        Some(host_arch()),
    )
    .unwrap();
    let foreign_rootfs = prepare_oci_rootfs(
        REMOTE_UBUNTU_REFERENCE,
        &runtime_prefix,
        &foreign_package_root,
        Some(&cache_root),
        Some(foreign_arch()),
    )
    .unwrap();

    let host_true = host_rootfs.join("usr/bin/true");
    let foreign_true = foreign_rootfs.join("usr/bin/true");
    assert_eq!(elf_machine(&host_true), expected_elf_machine(host_arch()));
    assert_eq!(
        elf_machine(&foreign_true),
        expected_elf_machine(foreign_arch())
    );
    assert_ne!(elf_machine(&host_true), elf_machine(&foreign_true));

    let mut cache_identities = fs::read_dir(&cache_root)
        .unwrap()
        .map(|entry| fs::read_to_string(entry.unwrap().path().join("reference.txt")).unwrap())
        .collect::<Vec<_>>();
    cache_identities.sort();
    assert_eq!(
        cache_identities.len(),
        2,
        "expected one cache entry per arch"
    );
    let normalized_reference = format!("docker://{REMOTE_UBUNTU_REFERENCE}");
    assert!(cache_identities.iter().any(|identity| identity
        == &format!(
            "{normalized_reference}
arch={}",
            expected_oci_arch(host_arch())
        )));
    assert!(cache_identities.iter().any(|identity| identity
        == &format!(
            "{normalized_reference}
arch={}",
            expected_oci_arch(foreign_arch())
        )));
}

#[test]
fn executes_foreign_arch_ubuntu_process_under_qemu_in_private_namespace() {
    if std::env::var_os(CHILD_MODE_ENV).as_deref() == Some(OsStr::new("foreign-run")) {
        let runtime_prefix =
            Path::new(&std::env::var("METALOR_TEST_RUNTIME_PREFIX").unwrap()).to_path_buf();
        let rootfs = PathBuf::from(std::env::var("METALOR_TEST_ROOTFS").unwrap());
        let work_dir = PathBuf::from(std::env::var("METALOR_TEST_WORK_DIR").unwrap());
        let emulator = std::env::var("METALOR_TEST_EMULATOR").unwrap();
        write_runtime_root_sentinel(&rootfs, &runtime_prefix);

        run_isolated_container_command(&ContainerRunCommand {
            root: rootfs,
            cwd: "/work".to_string(),
            mounts: vec![BindMount {
                source: work_dir,
                destination: "/work".to_string(),
                read_only: false,
            }],
            env: vec![("PATH".to_string(), "/usr/bin:/bin".to_string())],
            emulator: Some(emulator),
            executable: "/bin/sh".to_string(),
            argv: vec![
                "-lc".to_string(),
                "printf foreign > /work/result".to_string(),
            ],
        })
        .unwrap();
        return;
    }

    let runtime_prefix = unique_temp_dir("foreign-run-prefix");
    let package_root = runtime_prefix.join("foreign-pkg");
    let work_dir = unique_temp_dir("foreign-run-work");
    let rootfs = prepare_oci_rootfs(
        REMOTE_UBUNTU_REFERENCE,
        &runtime_prefix,
        &package_root,
        None,
        Some(foreign_arch()),
    )
    .unwrap();
    assert_eq!(
        elf_machine(&rootfs.join("usr/bin/true")),
        expected_elf_machine(foreign_arch())
    );
    let emulator = prepare_runtime_emulator(&rootfs, host_arch(), foreign_arch())
        .unwrap()
        .expect("foreign arch execution should require an emulator");

    let status = spawn_private_runtime_test(
        "executes_foreign_arch_ubuntu_process_under_qemu_in_private_namespace",
        &[
            (CHILD_MODE_ENV, "foreign-run"),
            (
                "METALOR_RUNTIME_ROOT_PREFIX",
                runtime_prefix.to_str().unwrap(),
            ),
            ("METALOR_PRIVATE_NS", "1"),
            (
                "METALOR_TEST_RUNTIME_PREFIX",
                runtime_prefix.to_str().unwrap(),
            ),
            ("METALOR_TEST_ROOTFS", rootfs.to_str().unwrap()),
            ("METALOR_TEST_WORK_DIR", work_dir.to_str().unwrap()),
            ("METALOR_TEST_EMULATOR", emulator.as_str()),
        ],
    );
    assert!(status.success(), "child test failed with {status}");
    assert_eq!(
        fs::read_to_string(work_dir.join("result")).unwrap(),
        "foreign"
    );
}

#[test]
fn reuses_cached_oci_layout_across_runtime_package_roots() {
    let runtime_prefix = unique_temp_dir("oci-cache-prefix");
    let cache_root = unique_temp_dir("oci-cache-root");
    let seed_layout = runtime_prefix.join("seed-layout");
    let package_root_a = runtime_prefix.join("pkg-a");
    let package_root_b = runtime_prefix.join("pkg-b");
    let reference = format!("oci:{}:base", seed_layout.display());

    run_host_command(
        Command::new("umoci")
            .args(["init", "--layout"])
            .arg(&seed_layout),
    );
    run_host_command(
        Command::new("umoci")
            .arg("new")
            .arg("--image")
            .arg(format!("{}:base", seed_layout.display())),
    );

    let rootfs_a = prepare_oci_rootfs(
        &reference,
        &runtime_prefix,
        &package_root_a,
        Some(&cache_root),
        None,
    )
    .unwrap();
    assert!(
        rootfs_a.is_dir(),
        "missing first rootfs at {}",
        rootfs_a.display()
    );

    let cache_entries = fs::read_dir(&cache_root)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect::<Vec<_>>();
    assert_eq!(cache_entries.len(), 1, "expected one cache entry");
    assert!(cache_entries[0].join("layout/index.json").is_file());

    fs::remove_dir_all(&seed_layout).unwrap();

    let rootfs_b = prepare_oci_rootfs(
        &reference,
        &runtime_prefix,
        &package_root_b,
        Some(&cache_root),
        None,
    )
    .unwrap();
    assert!(
        rootfs_b.is_dir(),
        "missing second rootfs at {}",
        rootfs_b.display()
    );
    assert!(rootfs_b.starts_with(package_root_b.canonicalize().unwrap()));
    assert!(!rootfs_b.starts_with(package_root_a.canonicalize().unwrap()));
}

#[test]
fn rejects_oci_package_roots_outside_the_runtime_prefix() {
    let runtime_prefix = unique_temp_dir("oci-prefix-outside");
    let other_prefix = unique_temp_dir("oci-package-outside");
    let seed_layout = runtime_prefix.join("seed-layout");

    run_host_command(
        Command::new("umoci")
            .args(["init", "--layout"])
            .arg(&seed_layout),
    );
    run_host_command(
        Command::new("umoci")
            .arg("new")
            .arg("--image")
            .arg(format!("{}:base", seed_layout.display())),
    );

    let error = prepare_oci_rootfs(
        &format!("oci:{}:base", seed_layout.display()),
        &runtime_prefix,
        &other_prefix.join("pkg"),
        None,
        None,
    )
    .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("must live under runtime root prefix"),
        "{error:#}"
    );
}

#[test]
fn rejects_oci_package_roots_through_symlinked_prefix_children_without_creating_them() {
    let runtime_prefix = unique_temp_dir("oci-symlink-prefix");
    let outside_root = unique_temp_dir("oci-symlink-outside");
    let link_path = runtime_prefix.join("link");
    let seed_layout = runtime_prefix.join("seed-layout");
    symlink(&outside_root, &link_path).unwrap();

    run_host_command(
        Command::new("umoci")
            .args(["init", "--layout"])
            .arg(&seed_layout),
    );
    run_host_command(
        Command::new("umoci")
            .arg("new")
            .arg("--image")
            .arg(format!("{}:base", seed_layout.display())),
    );

    let error = prepare_oci_rootfs(
        &format!("oci:{}:base", seed_layout.display()),
        &runtime_prefix,
        &link_path.join("pkg"),
        None,
        None,
    )
    .unwrap_err();
    assert!(
        error.to_string().contains("traverses a symlink")
            || error
                .to_string()
                .contains("must live under runtime root prefix"),
        "{error:#}"
    );
    assert!(
        !outside_root.join("pkg").exists(),
        "unexpected package-root creation through a symlinked prefix child"
    );
}

#[test]
fn rejects_bind_mount_targets_with_symlinked_parents_inside_private_namespace() {
    if std::env::var_os(CHILD_MODE_ENV).as_deref() == Some(OsStr::new("bind-symlink")) {
        let runtime_prefix =
            Path::new(&std::env::var("METALOR_TEST_RUNTIME_PREFIX").unwrap()).to_path_buf();
        let runtime_root = runtime_prefix.join("root");
        write_runtime_root_sentinel(&runtime_root, &runtime_prefix);
        let host_source =
            Path::new(&std::env::var("METALOR_TEST_HOST_SOURCE").unwrap()).to_path_buf();
        let error = run_isolated_container_command(&ContainerRunCommand {
            root: runtime_root,
            cwd: "/".to_string(),
            mounts: vec![BindMount {
                source: host_source,
                destination: "/usr".to_string(),
                read_only: true,
            }],
            env: vec![("PATH".to_string(), "/usr/bin:/bin".to_string())],
            emulator: None,
            executable: "/bin/true".to_string(),
            argv: Vec::new(),
        })
        .unwrap_err();
        assert!(
            error.to_string().contains("traverses a symlink"),
            "{error:#}"
        );
        return;
    }

    let runtime_prefix = unique_temp_dir("bind-mount-symlink-prefix");
    let runtime_root = runtime_prefix.join("root");
    let outside_root = unique_temp_dir("bind-mount-symlink-outside");
    let host_source = unique_temp_dir("bind-mount-source");
    fs::create_dir_all(&runtime_root).unwrap();
    symlink(&outside_root, runtime_root.join("usr")).unwrap();

    let status = spawn_private_runtime_test(
        "rejects_bind_mount_targets_with_symlinked_parents_inside_private_namespace",
        &[
            (CHILD_MODE_ENV, "bind-symlink"),
            (
                "METALOR_RUNTIME_ROOT_PREFIX",
                runtime_prefix.to_str().unwrap(),
            ),
            ("METALOR_PRIVATE_NS", "1"),
            (
                "METALOR_TEST_RUNTIME_PREFIX",
                runtime_prefix.to_str().unwrap(),
            ),
            ("METALOR_TEST_HOST_SOURCE", host_source.to_str().unwrap()),
        ],
    );
    assert!(status.success(), "child test failed with {status}");
}

#[test]
fn rejects_auto_mount_targets_with_symlinked_parents_inside_private_namespace() {
    if std::env::var_os(CHILD_MODE_ENV).as_deref() == Some(OsStr::new("auto-mount-symlink")) {
        let runtime_prefix =
            Path::new(&std::env::var("METALOR_TEST_RUNTIME_PREFIX").unwrap()).to_path_buf();
        let runtime_root = runtime_prefix.join("root");
        write_runtime_root_sentinel(&runtime_root, &runtime_prefix);
        let error = run_isolated_container_command(&ContainerRunCommand {
            root: runtime_root,
            cwd: "/".to_string(),
            mounts: Vec::new(),
            env: vec![("PATH".to_string(), "/usr/bin:/bin".to_string())],
            emulator: None,
            executable: "/bin/true".to_string(),
            argv: Vec::new(),
        })
        .unwrap_err();
        assert!(
            error.to_string().contains("traverses a symlink"),
            "{error:#}"
        );
        return;
    }

    let runtime_prefix = unique_temp_dir("auto-mount-symlink-prefix");
    let runtime_root = runtime_prefix.join("root");
    let outside_root = unique_temp_dir("auto-mount-symlink-outside");
    fs::create_dir_all(&runtime_root).unwrap();
    symlink(&outside_root, runtime_root.join("etc")).unwrap();

    let status = spawn_private_runtime_test(
        "rejects_auto_mount_targets_with_symlinked_parents_inside_private_namespace",
        &[
            (CHILD_MODE_ENV, "auto-mount-symlink"),
            (
                "METALOR_RUNTIME_ROOT_PREFIX",
                runtime_prefix.to_str().unwrap(),
            ),
            ("METALOR_PRIVATE_NS", "1"),
            (
                "METALOR_TEST_RUNTIME_PREFIX",
                runtime_prefix.to_str().unwrap(),
            ),
        ],
    );
    assert!(status.success(), "child test failed with {status}");
    assert!(
        !outside_root.join("resolv.conf").exists(),
        "unexpected host-side resolv.conf creation through a symlinked /etc"
    );
}
