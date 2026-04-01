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
    build_unshare_reexec_command, helper_binary_path, prepare_oci_rootfs, prepare_runtime_emulator,
    run_isolated_container_command, BindMount, ContainerRunCommand, RUN_HELPER_DIR,
};
