# metalor

Rust utilities for line-oriented DSL parsing and OCI-backed Linux runtime setup.

`metalor` is a small crate for trusted callers that want reusable low-level pieces instead of a full package manager or container runtime.

It provides:

- significant-line scanning with line numbers
- identifier validation
- JSON exec-array parsing
- `${NAME}` interpolation
- OCI copy/unpack helpers
- optional OCI layout caching
- requested-architecture image selection
- QEMU helper staging for foreign-arch execution
- guarded command execution inside a private mount namespace + `chroot`

## Add to your project

```toml
[dependencies]
metalor = "0.1"
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

## Runtime flow

The runtime API is intentionally split:

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
- rejecting host-side symlink traversal in reserved runtime paths before host-side mkdir/write/mount steps
- validating bind sources, mount destinations, `cwd`, executable paths, emulator paths, and environment entries before re-exec
- refusing to enter the inner runner from the host mount namespace

## Requirements

Linux only.

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

The runtime path assumes sufficient privilege to create mount namespaces, mount filesystems, and `chroot`.

## Non-goals

`metalor` is not a package manager, dependency resolver, build planner, full container runtime, or sandbox for hostile code.
