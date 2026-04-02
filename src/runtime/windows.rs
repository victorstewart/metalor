// Copyright 2026 Victor Stewart
// SPDX-License-Identifier: Apache-2.0

mod broker;
mod validation;
mod worker;

use super::BackendCaps;

pub use broker::{
    appcontainer_profile_name, build_worker_command, prepare_worker_request, WorkerTarget,
    WORKER_REQUEST_ENV,
};
pub use validation::{validate_application_id, validate_worker_target};
pub use worker::{
    build_worker_process_command, copy_worker_exports, load_request, load_request_from_env,
    prepare_job, sync_worker_caches, WorkerJob,
};

pub fn backend_caps() -> BackendCaps {
    BackendCaps {
        oci_rootfs: false,
        live_bind_mounts: false,
        foreign_arch_exec: false,
        per_job_network_toggle: true,
        profile_selected_network: false,
    }
}
