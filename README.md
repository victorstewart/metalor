# metalor

Small Rust primitives for line-oriented DSL parsing, portable build-cell orchestration, and OCI-backed Linux runtime setup.

`metalor` is a focused crate for tools like build systems, package managers, and image-driven executors that need reusable low-level layers without handing policy to a framework:

- parser helpers for simple, line-oriented config/build files
- a portable build-cell request/response layer built around explicit workspace seeds, imports, caches, and exports
- a narrow Linux runtime layer for preparing OCI rootfs trees and running commands inside a private mount namespace
- backend-specific consumer integration support for macOS helpers/XPC services and Windows worker brokers

The crate now supports Linux, macOS, and Windows natively, but not with a fake one-size-fits-all runtime:

- Linux gets native OCI/rootfs preparation plus verified private-namespace execution
- macOS gets native helper/XPC integration support for signed downstream helper targets
- Windows gets native broker/worker integration support for downstream worker processes

It is intentionally low-level and intentionally small. `metalor` handles the boring, easy-to-get-wrong pieces—filtered line scanning, JSON argv parsing, `${NAME}` interpolation, portable build-cell request files, OCI copy/unpack, architecture selection, QEMU staging, and guarded re-exec into a chrooted private runtime—without taking ownership of dependency resolution, build planning, artifact policy, or app signing.

## Why it exists

A lot of tools need the same substrate but should not share all of the same policy. `metalor` exists so callers can reuse:

- significant-line scanning with preserved line numbers
- identifier validation
- exec-form JSON array parsing
- `${NAME}` interpolation
- a portable build-cell spec with explicit workspace seeds, imports, caches, exports, env, limits, and cleanup policy
- a stable request/response file format for consumer-owned worker processes
- OCI copy/pull + unpack helpers
- optional OCI layout caching
- requested-architecture image selection
- QEMU helper staging for foreign-architecture execution
- a guarded outer/inner execution handoff for mount-namespace + `chroot` execution

Format-specific grammars, dependency semantics, build planning, and artifact logic stay in the owning tool.

## Public API at a glance

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

Linux portable execution:
- `build_cell_reexec_command`
- `run_build_cell`
- `finalize_build_cell`

Linux advanced runtime:
- `prepare_oci_rootfs`
- `prepare_runtime_emulator`
- `build_unshare_reexec_command`
- `run_isolated_container_command`
- `ContainerRunCommand`
- `BindMount`

macOS consumer integration:
- `runtime::macos::HelperTarget`
- `runtime::macos::prepare_helper_request`
- `runtime::macos::prepare_job`
- `runtime::macos::build_worker_command`
- `runtime::macos::*_TEMPLATE`

Windows consumer integration:
- `runtime::windows::WorkerTarget`
- `runtime::windows::prepare_worker_request`
- `runtime::windows::build_worker_command`
- `runtime::windows::prepare_job`
- `runtime::windows::build_worker_process_command`

## Portable build-cell model

The portable build-cell API is intentionally stricter than the Linux advanced runtime API.

- callers describe an ephemeral build job with `BuildCellSpec`
- host-side data enters through `WorkspaceSeed`, `ImportSpec`, and `CacheSpec`
- host-side outputs leave through `ExportSpec` and cache sync
- the shared contract is explicit staged I/O, not arbitrary live bind mounts

On Linux, `metalor` can execute that portable contract directly through the portable re-exec path:

1. The outer process prepares a `BuildCellSpec`.
2. The caller builds a re-exec command with `build_cell_reexec_command(...)`.
3. That re-execs the caller through `unshare` into a private namespace.
4. A private/internal subcommand reads the request and calls `run_build_cell(...)`.
5. After the worker exits, the outer process calls `finalize_build_cell(...)` to sync caches, export artifacts, and clean scratch state.

Networking is enabled by default in the portable policy surface. On Linux, disabling networking requests a private network namespace.

## Linux advanced runtime integration model

The runtime path is split in two on purpose.

1. The outer process prepares a `ContainerRunCommand`.
2. The caller builds a re-exec command with `build_unshare_reexec_command(...)`.
3. That re-execs the caller through `unshare` into a private mount namespace.
4. A private/internal subcommand in the caller reconstructs the request and calls `run_isolated_container_command(...)`.

Inside the isolated path, `metalor`:

- validates the request again before doing any mount or `chroot` work
- applies explicit bind mounts
- auto-binds a minimal host surface when not overridden:
  - `/etc/resolv.conf`
  - `/dev/null`
  - `/dev/zero`
  - `/dev/random`
  - `/dev/urandom`
- `chroot`s into the prepared rootfs
- clears the process environment and execs the target command with only the explicitly provided environment variables

That split is deliberate: the inner runner refuses to execute unless it can prove it is in a private mount namespace and the runtime root is a sentinel-marked path under the declared runtime prefix.

## Trust model

`metalor` is for trusted callers.

The caller is trusted to decide:
- what command to run
- what host paths to bind mount
- what OCI rootfs to prepare

`metalor` hardens the host-side runtime path by refusing a set of dangerous cases before host-side mkdir/write/mount operations happen. In the current implementation, it rejects:

- runtime roots and OCI package roots outside the declared runtime prefix
- host-side symlink traversal in reserved runtime paths
- relative bind sources or bind sources containing `..`
- invalid container `cwd`, executable paths, emulator paths, and mount destinations
- unsafe inner-runner entry from the host mount namespace

## Non-goals

`metalor` is not:

- a package manager
- a dependency resolver
- a build planner
- a full container runtime
- a sandbox for hostile code

The default re-exec path isolates mount, PID, UTS, and IPC namespaces and then `chroot`s into the prepared rootfs. It does not provide user-namespace isolation, network isolation, cgroup policy, seccomp policy, or a claim of safely executing arbitrary untrusted scripts.

## Platform model

`metalor` now has three backend surfaces:

- Linux:
  - verified OCI/chroot execution primitives
  - verified portable build-cell execution adapter
- macOS:
  - consumer-owned helper/XPC integration support
  - plist and entitlement templates
  - worker-side staging/execution helpers for signed helper targets
- Windows:
  - consumer-owned broker/worker integration support
  - worker request helpers and staged worker runtime helpers

`metalor` does not ship signed binaries. Downstream consumers own helper targets, entitlements, signing, notarization, embedding, and distribution.

Platform caveats:

- Linux is the only platform with the advanced OCI/rootfs + `unshare` + `chroot` execution path.
- macOS support is native, but it is shaped by Apple's sandbox model: downstream consumers need signed helper or XPC targets, and `metalor` provides the reusable broker/worker code plus templates rather than shipping those binaries itself.
- Windows support is native, but it is shaped around broker/worker process boundaries rather than Linux mount semantics; callers should use the portable build-cell model instead of expecting live bind mounts or Linux namespace behavior.

## Runtime requirements

The advanced OCI/runtime helpers are Linux-only.

The runtime helpers currently shell out to:

- `unshare`
- `mount`
- `umoci`
- `skopeo`

Foreign-architecture execution also requires the relevant `qemu-*-static` binary in `PATH`.

Currently supported architecture names are:

- `x86_64` / `amd64`
- `aarch64` / `arm64`
- `riscv64`

The Linux runtime path also assumes the caller already has the privilege required to create mount namespaces, perform mounts, and `chroot`.

## What is covered today

The test suite exercises:

- parser edge cases
- portable build-cell request construction
- portable build-cell staging, cache sync, and export sync under a real private Linux namespace
- re-exec command construction
- runtime-root confinement and sentinel enforcement
- host-side symlink rejection for runtime roots, package roots, helper staging, bind targets, and auto-mount targets
- successful in-namespace execution with explicit auto-mount overrides
- OCI unpack from both local OCI layouts and a pinned remote Ubuntu image
- requested-architecture OCI selection and cache partitioning
- real foreign-architecture execution with staged QEMU

## Status

`metalor` is intentionally narrow, safety-biased, and built for reuse by trusted tooling. Today that means:

- portable build-cell specs and worker protocol for multi-OS integrations
- consumer-facing helper support for macOS and Windows
- fully verified advanced execution on Linux

If you need small, auditable low-level primitives rather than a policy-heavy container platform, this is the layer it is trying to be.
