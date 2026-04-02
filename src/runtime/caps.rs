// Copyright 2026 Victor Stewart
// SPDX-License-Identifier: Apache-2.0

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BackendCaps {
    pub oci_rootfs: bool,
    pub live_bind_mounts: bool,
    pub foreign_arch_exec: bool,
    pub per_job_network_toggle: bool,
    pub profile_selected_network: bool,
}
