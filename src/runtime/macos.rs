// Copyright 2026 Victor Stewart
// SPDX-License-Identifier: Apache-2.0

mod assets;
mod client;
mod linux_provider;
mod validation;
mod worker;

use super::BackendCaps;

pub use assets::{
    HELPER_INFO_PLIST_TEMPLATE, NETWORKED_HELPER_ENTITLEMENTS_TEMPLATE,
    OFFLINE_HELPER_ENTITLEMENTS_TEMPLATE,
};
pub use client::{helper_environment, prepare_helper_request, HelperTarget, HELPER_REQUEST_ENV};
pub use linux_provider::{AppleLinuxProvider, DEFAULT_APPLE_LINUX_BUNDLE};
pub use validation::{validate_bundle_identifier, validate_helper_target, validate_service_name};
pub use worker::{
    build_worker_command, copy_worker_exports, load_request, load_request_from_env, prepare_job,
    sync_worker_caches, WorkerJob,
};

pub fn backend_caps() -> BackendCaps {
    BackendCaps {
        oci_rootfs: false,
        live_bind_mounts: false,
        foreign_arch_exec: false,
        per_job_network_toggle: false,
        profile_selected_network: true,
    }
}
