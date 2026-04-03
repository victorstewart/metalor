# metalor

Small Rust primitives for line-oriented DSL parsing, portable build-cell orchestration, local Linux-provider integration, and OCI-backed Linux runtime setup.

`metalor` is for trusted build tools that need reusable runtime substrate without inheriting a package manager, build planner, or policy-heavy container framework.

It gives downstream tools four layers:

- parser helpers for simple line-oriented config and build files
- a portable `BuildCellSpec` protocol built around explicit workspace seeds, imports, caches, and exports
- a Linux-only advanced runtime for OCI/rootfs preparation and private-namespace execution
- native non-Linux integration layers for macOS and Windows, including local Linux-provider hooks

## Platform model

- Linux:
  - full OCI/rootfs preparation
  - private `unshare` + `chroot` execution
  - QEMU staging for foreign-architecture execution
  - direct portable build-cell execution
- macOS:
  - helper/XPC request prep, worker staging, and template assets
  - `runtime::macos::AppleLinuxProvider` for downstream tools that manage a local Linux VM through a caller-owned helper
- Windows:
  - broker/worker request prep and staged worker helpers
  - `runtime::windows::WslProvider` and `resolve_wsl_distro(...)` for downstream tools that want a local Linux provider through WSL2
- Cross-platform:
  - `BuildCellSpec` is the stable staged-I/O contract
  - Linux mount-namespace behavior is not faked on macOS or Windows

## What Changed

`metalor` now has a real local Linux-provider substrate for non-Linux hosts.

- Linux still owns the advanced OCI/rootfs runtime.
- macOS and Windows still own their native worker/helper integration layers.
- Downstream tools can now reuse a shared provider session/runtime layer instead of rebuilding:
  - provider selection and validation
  - provider runtime layout and metadata files
  - staged path sync into and out of a local Linux environment
  - warm/cold provider bootstrap tracking

That means callers can keep one portable build-cell model while routing Linux-rootfs work through:

- `runtime::windows::WslProvider` on Windows
- `runtime::macos::AppleLinuxProvider` on macOS

## Public API At A Glance

Parser:

- `significant_lines`
- `valid_identifier`
- `parse_exec_array`
- `interpolate_braced_variables`

Portable build-cell layer:

- `BackendCaps`
- `BuildCellSpec`
- `BuildCellResult`
- `build_cell_request_path`
- `read_build_cell_request`
- `write_build_cell_request`

Local Linux-provider layer:

- `runtime::linux_provider::LocalLinuxProviderSelection`
- `runtime::linux_provider::LocalLinuxProviderKind`
- `runtime::linux_provider::ProviderRuntimeLayout`
- `runtime::linux_provider::ProviderRuntimeMetadata`
- `runtime::linux_provider::ProviderSession`
- `runtime::linux_provider::ProviderShell`

Linux advanced runtime:

- `build_cell_reexec_command`
- `run_build_cell`
- `finalize_build_cell`
- `prepare_oci_rootfs`
- `prepare_runtime_emulator`
- `build_unshare_reexec_command`
- `run_isolated_container_command`
- `ContainerRunCommand`
- `BindMount`

macOS integration:

- `runtime::macos::HelperTarget`
- `runtime::macos::prepare_helper_request`
- `runtime::macos::helper_environment`
- `runtime::macos::build_worker_command`
- `runtime::macos::prepare_job`
- `runtime::macos::sync_worker_caches`
- `runtime::macos::copy_worker_exports`
- `runtime::macos::AppleLinuxProvider`
- `runtime::macos::*_TEMPLATE`

Windows integration:

- `runtime::windows::WorkerTarget`
- `runtime::windows::prepare_worker_request`
- `runtime::windows::build_worker_command`
- `runtime::windows::prepare_job`
- `runtime::windows::build_worker_process_command`
- `runtime::windows::sync_worker_caches`
- `runtime::windows::copy_worker_exports`
- `runtime::windows::WslProvider`
- `runtime::windows::resolve_wsl_distro`
- `runtime::windows::DEFAULT_WSL_DISTRO`

## Behavioral Model

- The portable build-cell layer is the cross-platform contract: explicit staged inputs, explicit staged outputs, no ambient live host mounts.
- The advanced OCI/rootfs runtime stays Linux-only. `metalor` does not pretend macOS or Windows can directly provide Linux namespace semantics.
- On macOS and Windows, callers that need Linux OCI/rootfs behavior should route jobs through a local Linux provider instead of expecting live bind mounts or `unshare` parity.
- `metalor` stays low-level. It does not own dependency resolution, build planning, package policy, helper signing, notarization, VM image policy, or downstream release packaging.

## Trust Model

`metalor` is for trusted callers.

The caller is trusted to decide:

- what command to run
- what OCI rootfs to prepare
- what local Linux provider to use
- what host paths to stage or mount

`metalor` hardens the runtime path by rejecting a class of dangerous cases before host-side mkdir, write, mount, or sync operations happen. In the current implementation it rejects:

- runtime roots and OCI package roots outside the declared runtime prefix
- host-side symlink traversal in reserved runtime paths
- relative bind sources or bind sources containing `..`
- invalid container `cwd`, executable paths, emulator paths, and mount destinations
- unsafe inner-runner entry from the host mount namespace

## Runtime Requirements

The advanced OCI/runtime helpers are Linux-only.

Those helpers currently shell out to:

- `unshare`
- `mount`
- `umoci`
- `skopeo`

Foreign-architecture execution also requires the relevant `qemu-*-static` binary in `PATH`.

Supported architecture names:

- `x86_64` / `amd64`
- `aarch64` / `arm64`
- `riscv64`

The Linux advanced runtime path assumes the caller already has the privilege required to create mount namespaces, perform mounts, and `chroot`.

For non-Linux Linux providers:

- Windows support assumes WSL2 is available
- macOS support assumes a downstream-owned helper that can create or resume a Linux VM and service `ensure` / `shell` requests

`metalor` does not ship signed helpers, VM images, or a turnkey VM manager.

## Non-goals

`metalor` is not:

- a package manager
- a dependency resolver
- a build planner
- a full container runtime
- a sandbox for hostile code
- a signed macOS helper bundle
- a Windows installer or broker service
- a VM distribution system

## Status

`metalor` is intentionally narrow, safety-biased, and built for reuse by trusted tooling.

Today that means:

- parser primitives for line-oriented DSLs
- portable build-cell specs and worker protocol for multi-OS integrations
- reusable local Linux-provider substrate for macOS and Windows callers
- fully verified advanced OCI/rootfs execution on Linux

If you need small, auditable low-level primitives rather than a policy-heavy build platform, this is the layer it is trying to be.
