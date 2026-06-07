//! Typed, SDK-owned planning projections for the offline dry-run / dump.
//!
//! The facade's network-free dump ([`modal_rust::dump`]) needs to project exactly
//! what the wire WOULD carry for the two load-bearing requests — the image's
//! `dockerfile_commands` and the FILE-mode `FunctionCreate` fields — WITHOUT
//! re-implementing the request shapes (which would let the dump drift from the live
//! path). Previously the SDK re-exported the raw `build_*_request` functions, which
//! return RAW proto types, so the facade reached across the crate boundary into
//! `modal.client` proto. That leaked proto into the SDK's public API.
//!
//! This module closes that leak: [`plan_image_request`] / [`plan_function_request`]
//! call the SAME internal `build_*_request` builders the live ops call (so there is
//! still ONE seam and ZERO drift), but PROJECT the result into SDK-OWNED plain
//! structs ([`PlannedImage`] / [`PlannedFunction`]). No proto type crosses the
//! crate boundary. The projected fields are exactly the values the corresponding
//! proto message carries on the wire.

use crate::ops::function::{build_function_create_request, FunctionSpec};
use crate::ops::image::{build_image_get_or_create_request, ImageSpec};

/// SDK-owned projection of an `ImageGetOrCreate` request, carrying just the fields
/// the dump renders. Built from [`plan_image_request`]; no proto leaks out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedImage {
    /// The rendered `dockerfile_commands` (the exact list the wire carries) — the
    /// SAME `Vec<String>` `ImageSpec::to_proto` produces.
    pub dockerfile_commands: Vec<String>,
}

/// SDK-owned projection of a FILE-mode `FunctionCreate` request, carrying just the
/// fields the dump renders/asserts. Built from [`plan_function_request`]; no proto
/// leaks out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedFunction {
    /// The importable wrapper module name (`Function.module_name`).
    pub module_name: String,
    /// The Modal app-namespace object TAG (`Function.function_name` — the entrypoint
    /// name once decoupled from the in-container callable).
    pub function_name: String,
    /// Number of attached mount ids (`Function.mount_ids.len()`).
    pub mount_ids_count: usize,
    /// The GPU type, if any (`Function.resources.gpu_config.gpu_type`); `None` = CPU.
    pub gpu: Option<String>,
    /// Requested CPU in milli-cores (`Function.resources.milli_cpu`); `0` = server
    /// default.
    pub milli_cpu: u32,
    /// Requested memory in MiB (`Function.resources.memory_mb`); `0` = server default.
    pub memory_mb: u32,
    /// The function timeout in seconds (`Function.timeout_secs`).
    pub timeout_secs: u32,
    /// Volume mounts as `(mount_path, volume_id)` pairs (`Function.volume_mounts`).
    pub volume_mounts: Vec<(String, String)>,
    /// Number of attached secret ids (`Function.secret_ids.len()`).
    pub secret_ids_count: usize,
    /// The FILE-mode XOR invariant: `FunctionCreateRequest.function_data` is unset.
    pub function_data_is_none: bool,
}

/// Project the `ImageGetOrCreate` request the live image op WOULD send for `spec`
/// (under `app_id` + `builder_version`) into an SDK-owned [`PlannedImage`]. Built ON
/// the SAME [`build_image_get_or_create_request`] the live path calls, so the
/// projected `dockerfile_commands` are exactly what the wire would carry.
pub fn plan_image_request(spec: &ImageSpec, app_id: &str, builder_version: &str) -> PlannedImage {
    let req = build_image_get_or_create_request(spec, app_id, builder_version.to_string());
    PlannedImage {
        dockerfile_commands: req.image.map(|i| i.dockerfile_commands).unwrap_or_default(),
    }
}

/// Project the FILE-mode `FunctionCreate` request the live function op WOULD send for
/// `spec` (under `app_id` + `precreate_id`) into an SDK-owned [`PlannedFunction`].
/// Built ON the SAME [`build_function_create_request`] the live path calls, so the
/// projected fields are exactly what the wire would carry (including the
/// object-tag/implementation-name decoupling and the `function_data` XOR).
pub fn plan_function_request(
    app_id: &str,
    precreate_id: &str,
    spec: &FunctionSpec,
) -> PlannedFunction {
    let req = build_function_create_request(app_id, precreate_id, spec);
    let function_data_is_none = req.function_data.is_none();
    let function = req.function.expect("FILE-mode sets `function`");
    let gpu = function
        .resources
        .as_ref()
        .and_then(|r| r.gpu_config.as_ref())
        .map(|g| g.gpu_type.clone());
    let milli_cpu = function
        .resources
        .as_ref()
        .map(|r| r.milli_cpu)
        .unwrap_or(0);
    let memory_mb = function
        .resources
        .as_ref()
        .map(|r| r.memory_mb)
        .unwrap_or(0);
    let volume_mounts = function
        .volume_mounts
        .iter()
        .map(|m| (m.mount_path.clone(), m.volume_id.clone()))
        .collect();
    PlannedFunction {
        module_name: function.module_name.clone(),
        function_name: function.function_name.clone(),
        mount_ids_count: function.mount_ids.len(),
        gpu,
        milli_cpu,
        memory_mb,
        timeout_secs: function.timeout_secs,
        volume_mounts,
        secret_ids_count: function.secret_ids.len(),
        function_data_is_none,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_image_projects_dockerfile_commands_from_the_builder() {
        let spec = ImageSpec::from_registry("rust:1-slim")
            .with_add_python("3.12")
            .with_python_standalone_mount_id("mo-py")
            .with_wrapper_module(
                "modal_rust_run_wrapper",
                "def handler(e, i):\n    return i\n",
            );
        let planned = plan_image_request(&spec, "ap-1", "2025.06");
        // Same first line + add_python COPY the raw proto would carry.
        assert_eq!(planned.dockerfile_commands[0], "FROM rust:1-slim");
        assert!(planned
            .dockerfile_commands
            .iter()
            .any(|c| c == "COPY /python/. /usr/local"));
    }

    #[test]
    fn plan_function_projects_file_mode_fields_and_xor() {
        let spec = FunctionSpec::new("modal_rust_run_wrapper", "handler", "im-1")
            .with_app_function_name("add")
            .with_mount_ids(vec!["mo-1".to_string(), "mo-2".to_string()])
            .with_timeout_secs(1800)
            .with_gpu(Some("T4"))
            .expect("valid gpu")
            .with_milli_cpu(Some(2000))
            .with_memory_mb(Some(4096));
        let planned = plan_function_request("ap-1", "fu-pre-1", &spec);
        assert_eq!(planned.module_name, "modal_rust_run_wrapper");
        // Object tag = the entrypoint ("add"), decoupled from the "handler" callable.
        assert_eq!(planned.function_name, "add");
        assert_eq!(planned.mount_ids_count, 2);
        assert_eq!(planned.gpu.as_deref(), Some("T4"));
        assert_eq!(planned.milli_cpu, 2000);
        assert_eq!(planned.memory_mb, 4096);
        assert_eq!(planned.timeout_secs, 1800);
        assert_eq!(planned.secret_ids_count, 0);
        assert!(
            planned.function_data_is_none,
            "FILE-mode XOR: function_data is None"
        );
    }
}
