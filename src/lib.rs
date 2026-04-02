// Copyright 2026 Victor Stewart
// SPDX-License-Identifier: Apache-2.0

//! Generic parsing utilities shared across build-file formats.

pub mod parser;
pub mod runtime;
pub use parser::{
    interpolate_braced_variables, parse_exec_array, significant_lines, valid_identifier,
    SignificantLine,
};
pub use runtime::{
    backend_caps, build_cell_request_path, read_build_cell_request, write_build_cell_request,
    BackendCaps, BuildCellResult, BuildCellSpec, CacheSpec, CellPath, CleanupPolicy, CommandSpec,
    ExportSpec, HostPath, ImportSpec, NetworkPolicy, ResourceLimits, WorkspaceSeed,
};

#[cfg(target_os = "linux")]
pub use runtime::{
    build_cell_reexec_command, build_unshare_reexec_command, finalize_build_cell,
    helper_binary_path, prepare_oci_rootfs, prepare_runtime_emulator, run_build_cell,
    run_isolated_container_command, BindMount, ContainerRunCommand, RUN_HELPER_DIR,
};
