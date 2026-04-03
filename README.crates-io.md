# metalor

Rust utilities for line-oriented DSL parsing, portable build-cell orchestration, local Linux-provider integration, and OCI-backed Linux runtime setup.

`metalor` is a small crate for trusted tools that want reusable runtime substrate without adopting a full package manager, build planner, or container runtime.

It provides:

- parser helpers for line-oriented config and build files
- a portable `BuildCellSpec` request/response layer with explicit staged imports, caches, and exports
- generic local Linux-provider helpers under `runtime::linux_provider`
- Linux advanced OCI/rootfs + private-namespace execution on Linux
- native macOS helper/XPC worker support plus `runtime::macos::AppleLinuxProvider`
- native Windows broker/worker support plus `runtime::windows::WslProvider`

## Add to your project

```toml
[dependencies]
metalor = "0.3"
```

## Parser example

```rust
use metalor::{interpolate_braced_variables, parse_exec_array, significant_lines};
use std::collections::BTreeMap;

# fn main() -> Result<(), Box<dyn std::error::Error>> {
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
# Ok(())
# }
```

## Platform model

- `BuildCellSpec` is the cross-platform staged-I/O contract.
- Linux has the full OCI/rootfs + `unshare` + `chroot` runtime path.
- macOS has native helper/worker support plus a caller-owned Apple Linux-provider hook.
- Windows has native broker/worker support plus a WSL2-backed Linux-provider hook.
- Linux namespace behavior is not emulated on macOS or Windows. Callers that need Linux OCI/rootfs behavior there should route jobs through a local Linux provider.

## Linux-provider surface

The new local Linux-provider layer is intentionally small and reusable:

- `runtime::linux_provider::ProviderSession`
- `runtime::linux_provider::ProviderRuntimeLayout`
- `runtime::linux_provider::ProviderRuntimeMetadata`
- `runtime::windows::WslProvider`
- `runtime::windows::resolve_wsl_distro`
- `runtime::macos::AppleLinuxProvider`

This layer is for downstream tools that want to keep one portable build-cell model while delegating Linux-rootfs work to a local Linux environment.

## Linux runtime flow

The advanced Linux runtime API is intentionally split:

1. `prepare_oci_rootfs(...)` copies or unpacks a rootfs under a declared runtime prefix.
2. `prepare_runtime_emulator(...)` stages `qemu-*-static` under `/.metalor-run` when host and guest architectures differ.
3. `build_unshare_reexec_command(...)` constructs the outer `unshare` re-exec.
4. Your private/internal subcommand reconstructs the request and calls `run_isolated_container_command(...)`.

The executed process gets a cleared environment. Pass `PATH` and any other required variables explicitly in `ContainerRunCommand::env`.

If you do not override them yourself, `metalor` auto-binds a minimal host surface for the isolated command:

- `/etc/resolv.conf`
- `/dev/null`
- `/dev/zero`
- `/dev/random`
- `/dev/urandom`

## Safety model

`metalor` assumes a trusted caller, but it hardens the host-side runtime path by:

- keeping runtime roots and OCI package roots under a declared runtime prefix
- rejecting host-side symlink traversal in reserved runtime paths before host-side mkdir, write, mount, or sync steps
- validating bind sources, mount destinations, `cwd`, executable paths, emulator paths, and environment entries before re-exec
- refusing to enter the inner runner from the host mount namespace

## Requirements

The advanced OCI/runtime helpers are Linux-only.

Runtime helpers currently rely on:

- `unshare`
- `mount`
- `umoci`
- `skopeo`

Foreign-architecture execution also requires the matching `qemu-*-static` binary in `PATH`.

Supported architecture names:

- `x86_64` / `amd64`
- `aarch64` / `arm64`
- `riscv64`

The Linux advanced runtime path assumes sufficient privilege to create mount namespaces, mount filesystems, and `chroot`.

## Non-goals

`metalor` is not a package manager, dependency resolver, build planner, full container runtime, or sandbox for hostile code. It also does not ship signed macOS helpers, VM images, or downstream app packaging.
