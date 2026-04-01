# metalor

Small Rust primitives for line-oriented DSL parsing and OCI-backed Linux runtime setup.

`metalor` is a focused crate for tools like build systems, package managers, and image-driven executors that need two reusable layers:

- parser helpers for simple, line-oriented config/build files
- a narrow Linux runtime layer for preparing OCI rootfs trees and running commands inside a private mount namespace

It is intentionally low-level and intentionally small. `metalor` handles the boring, easy-to-get-wrong pieces—filtered line scanning, JSON argv parsing, `${NAME}` interpolation, OCI copy/unpack, architecture selection, QEMU staging, and guarded re-exec into a chrooted private runtime—without taking ownership of dependency resolution, build planning, or artifact policy.

## Why it exists

A lot of tools need the same substrate but should not share all of the same policy. `metalor` exists so callers can reuse:

- significant-line scanning with preserved line numbers
- identifier validation
- exec-form JSON array parsing
- `${NAME}` interpolation
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

Runtime:
- `prepare_oci_rootfs`
- `prepare_runtime_emulator`
- `build_unshare_reexec_command`
- `run_isolated_container_command`
- `ContainerRunCommand`
- `BindMount`

## Runtime integration model

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

## Runtime requirements

`metalor` is Linux-only.

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

The runtime path also assumes the caller already has the privilege required to create mount namespaces, perform mounts, and `chroot`.

## What is covered today

The test suite exercises:

- parser edge cases
- re-exec command construction
- runtime-root confinement and sentinel enforcement
- host-side symlink rejection for runtime roots, package roots, helper staging, bind targets, and auto-mount targets
- successful in-namespace execution with explicit auto-mount overrides
- OCI unpack from both local OCI layouts and a pinned remote Ubuntu image
- requested-architecture OCI selection and cache partitioning
- real foreign-architecture execution with staged QEMU

## Status

`metalor` is intentionally narrow, safety-biased, and built for reuse by trusted Linux tooling. If you need small, auditable low-level primitives rather than a policy-heavy container platform, this is the layer it is trying to be.
