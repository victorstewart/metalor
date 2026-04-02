// Copyright 2026 Victor Stewart
// SPDX-License-Identifier: Apache-2.0

pub const HELPER_INFO_PLIST_TEMPLATE: &str = include_str!("templates/MetalorWorker-Info.plist");
pub const NETWORKED_HELPER_ENTITLEMENTS_TEMPLATE: &str =
    include_str!("templates/MetalorWorker-networked.entitlements");
pub const OFFLINE_HELPER_ENTITLEMENTS_TEMPLATE: &str =
    include_str!("templates/MetalorWorker-offline.entitlements");
