// Copyright 2026 Victor Stewart
// SPDX-License-Identifier: Apache-2.0

mod caps;
mod portable;
mod protocol;
#[cfg(any(target_os = "macos", target_os = "windows"))]
mod worker_support;

pub use caps::BackendCaps;
pub use portable::{
    BuildCellResult, BuildCellSpec, CacheSpec, CellPath, CleanupPolicy, CommandSpec, ExportSpec,
    HostPath, ImportSpec, NetworkPolicy, ResourceLimits, WorkspaceSeed,
};
pub use protocol::{build_cell_request_path, read_build_cell_request, write_build_cell_request};

#[cfg(target_os = "linux")]
pub mod linux;
#[cfg(target_os = "linux")]
pub use linux::{
    backend_caps, build_cell_reexec_command, build_unshare_reexec_command, finalize_build_cell,
    helper_binary_path, prepare_oci_rootfs, prepare_runtime_emulator, run_build_cell,
    run_isolated_container_command, BindMount, ContainerRunCommand, RUN_HELPER_DIR,
};

#[cfg(target_os = "macos")]
pub mod macos;
#[cfg(target_os = "macos")]
pub use macos::backend_caps;

#[cfg(target_os = "windows")]
pub mod windows;
#[cfg(target_os = "windows")]
pub use windows::backend_caps;

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
compile_error!("metalor runtime backends currently support only linux, macos, and windows");
