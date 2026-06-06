//! App operations: `AppGetOrCreate` (preferred) / `AppCreate` (ephemeral) and
//! `AppPublish` (deploy).
//!
//! ## Fix #2 — deploy via `AppPublish` ONLY
//!
//! The legacy `AppSetObjects` RPC handler is server-broken
//! (`module 'grpc' has no attribute 'experimental'`). Modern Modal deploys with
//! `AppPublish` alone (mirrors Python `runner._publish_app`). We never call
//! `AppSetObjects` (see `workpads/shim-backend/spike-notes.md`).

use std::collections::HashMap;

use crate::client::ModalClient;
use crate::error::Result;
use crate::proto::api::{
    AppCreateRequest, AppGetOrCreateRequest, AppPublishRequest, AppState, ObjectCreationType,
};

/// Build the `AppGetOrCreate` request (api.proto:4142) — pure, no I/O.
///
/// Extracted from [`ModalClient::app_get_or_create_id`] so the assembled request is
/// unit-testable offline; the method passes the resolved `environment_name` (it
/// resolves it via `env_or_default` exactly as before). Created with
/// `OBJECT_CREATION_TYPE_CREATE_IF_MISSING` semantics.
pub(crate) fn build_app_get_or_create_request(
    app_name: &str,
    environment_name: String,
) -> AppGetOrCreateRequest {
    AppGetOrCreateRequest {
        app_name: app_name.to_string(),
        environment_name,
        object_creation_type: ObjectCreationType::CreateIfMissing as i32,
    }
}

/// Build the `AppCreate` request (api.proto:4133) for an EPHEMERAL app — pure, no I/O.
///
/// Extracted from [`ModalClient::app_create_ephemeral`]; the method passes the
/// resolved `environment_name`. `app_state` is `APP_STATE_EPHEMERAL` (GC'd when the
/// client disconnects).
pub(crate) fn build_app_create_request(
    description: &str,
    environment_name: String,
) -> AppCreateRequest {
    AppCreateRequest {
        client_id: String::new(),
        description: description.to_string(),
        environment_name,
        app_state: AppState::Ephemeral as i32,
        tags: HashMap::new(),
    }
}

/// Build the `AppPublish` request (api.proto:4147) — **fix #2**, pure, no I/O.
///
/// Extracted from [`ModalClient::app_publish`]. `app_state` decides persistence
/// (`Ephemeral` on the RUN path, `Deployed` on the DEPLOY path); the
/// `function_ids` / `definition_ids` maps ride through unchanged.
pub(crate) fn build_app_publish_request(
    app_id: &str,
    app_name: &str,
    app_state: AppState,
    function_ids: HashMap<String, String>,
    definition_ids: HashMap<String, String>,
) -> AppPublishRequest {
    AppPublishRequest {
        app_id: app_id.to_string(),
        name: app_name.to_string(),
        app_state: app_state as i32,
        function_ids,
        definition_ids,
        ..Default::default()
    }
}

/// Outcome of a successful [`ModalClient::app_publish`] (deploy).
#[derive(Debug, Clone, Default)]
pub struct PublishedApp {
    /// Deployed app URL (may be empty depending on server response).
    pub url: String,
    /// Server-side deploy timestamp (epoch seconds, fractional).
    pub deployed_at: f64,
    /// Advisory server warnings emitted by the publish (rendered text).
    pub warnings: Vec<String>,
}

impl ModalClient {
    /// `AppGetOrCreate` (api.proto:4142) — idempotent, resume-friendly. Returns
    /// the `app_id` threaded through the rest of the recipe.
    ///
    /// `environment` defaults to the configured environment (or `"main"`). Created
    /// with `OBJECT_CREATION_TYPE_CREATE_IF_MISSING` semantics.
    ///
    /// This is also the cheapest safe live auth proof on the ops surface (free, no
    /// GPU). It is identical to [`ModalClient::app_get_or_create`] on the client —
    /// retained here for the ops grouping; prefer the inherent method.
    pub async fn app_get_or_create_id(
        &mut self,
        app_name: &str,
        environment: Option<&str>,
    ) -> Result<String> {
        let environment_name = self.env_or_default(environment);
        let req = build_app_get_or_create_request(app_name, environment_name);
        let resp = self
            .retry_rpc("app_get_or_create", req, |mut stub, req| async move {
                stub.app_get_or_create(req).await
            })
            .await?;
        Ok(resp.app_id)
    }

    /// `AppCreate` (api.proto:4133) for an **ephemeral** app — discharged when the
    /// client disconnects. Returns the new `app_id`.
    ///
    /// Use this for one-shot, throwaway invocations; prefer
    /// [`ModalClient::app_get_or_create_id`] for deploy flows.
    pub async fn app_create_ephemeral(
        &mut self,
        description: &str,
        environment: Option<&str>,
    ) -> Result<String> {
        let environment_name = self.env_or_default(environment);
        // NOTE: not on the run path (which uses get-or-create). A dropped response
        // after a transient reset could in principle create a duplicate ephemeral
        // app, but ephemerals are GC'd when the client disconnects, so retrying is
        // acceptable (per the resilience spec A.5).
        let req = build_app_create_request(description, environment_name);
        let resp = self
            .retry_rpc("app_create", req, |mut stub, req| async move {
                stub.app_create(req).await
            })
            .await?;
        Ok(resp.app_id)
    }

    /// Publish an app's functions via `AppPublish` (api.proto:4147) — **fix #2**.
    ///
    /// - `function_ids`: `function_name` → `function_id`.
    /// - `definition_ids`: `function_id` → `definition_id`
    ///   (from `FunctionCreateResponse.handle_metadata.definition_id`).
    /// - `app_state`: the state the published app enters.
    ///
    /// AppPublish is REQUIRED to make a created function INVOKABLE (without it,
    /// `FunctionMap` fails "function ... not found" — live-verified 2026-06-04).
    /// The `app_state` decides persistence, mirroring Modal Python's `runner.py`
    /// (which publishes BOTH ephemeral runs and deploys, differing only in state):
    ///
    /// - [`AppState::Ephemeral`] — the RUN path. The app is "discharged when the
    ///   client disconnects" (proto), so `.remote()` leaves NO lingering deploy.
    /// - [`AppState::Deployed`] — the DEPLOY path. Persistent; `from_name` resolves
    ///   it; re-deploys REPLACE under the same name.
    ///
    /// Prefer the intent-named wrappers [`ModalClient::app_publish_ephemeral`] /
    /// [`ModalClient::app_publish_deployed`]. We never issue the legacy
    /// `AppSetObjects` RPC.
    pub async fn app_publish(
        &mut self,
        app_id: &str,
        app_name: &str,
        app_state: AppState,
        function_ids: HashMap<String, String>,
        definition_ids: HashMap<String, String>,
    ) -> Result<PublishedApp> {
        // AppPublish is a set-state publish (not an append): re-publishing the same
        // function_ids/definition_ids is idempotent, so retrying on a transient
        // reset is safe.
        let req =
            build_app_publish_request(app_id, app_name, app_state, function_ids, definition_ids);
        let resp = self
            .retry_rpc("app_publish", req, |mut stub, req| async move {
                stub.app_publish(req).await
            })
            .await?;

        Ok(PublishedApp {
            url: resp.url,
            deployed_at: resp.deployed_at,
            warnings: resp
                .server_warnings
                .iter()
                .map(|w| w.message.clone())
                .collect(),
        })
    }

    /// Publish into [`AppState::Ephemeral`] — the RUN path. Makes the created
    /// function INVOKABLE while keeping the app throwaway (GC'd when the client
    /// disconnects), so `.remote()` leaves no lingering deploy.
    pub async fn app_publish_ephemeral(
        &mut self,
        app_id: &str,
        app_name: &str,
        function_ids: HashMap<String, String>,
        definition_ids: HashMap<String, String>,
    ) -> Result<PublishedApp> {
        self.app_publish(
            app_id,
            app_name,
            AppState::Ephemeral,
            function_ids,
            definition_ids,
        )
        .await
    }

    /// Publish into [`AppState::Deployed`] — the DEPLOY path. Persistent;
    /// `from_name` resolves it and re-deploys REPLACE under the same name.
    pub async fn app_publish_deployed(
        &mut self,
        app_id: &str,
        app_name: &str,
        function_ids: HashMap<String, String>,
        definition_ids: HashMap<String, String>,
    ) -> Result<PublishedApp> {
        self.app_publish(
            app_id,
            app_name,
            AppState::Deployed,
            function_ids,
            definition_ids,
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_app_get_or_create_is_create_if_missing() {
        let req = build_app_get_or_create_request("my-app", "main".to_string());
        assert_eq!(req.app_name, "my-app");
        assert_eq!(req.environment_name, "main");
        // CREATE_IF_MISSING (1) — idempotent, resume-friendly.
        assert_eq!(
            req.object_creation_type,
            ObjectCreationType::CreateIfMissing as i32
        );
        assert_eq!(req.object_creation_type, 1);
    }

    #[test]
    fn build_app_create_is_ephemeral() {
        let req = build_app_create_request("a description", "main".to_string());
        assert_eq!(req.description, "a description");
        assert_eq!(req.environment_name, "main");
        // The RUN-path ephemeral app is GC'd on disconnect.
        assert_eq!(req.app_state, AppState::Ephemeral as i32);
        // No client id / tags set (wire-identical to the inline literal).
        assert!(req.client_id.is_empty());
        assert!(req.tags.is_empty());
    }

    #[test]
    fn build_app_publish_projects_state_and_maps() {
        let mut function_ids = HashMap::new();
        function_ids.insert("handler".to_string(), "fu-1".to_string());
        let mut definition_ids = HashMap::new();
        definition_ids.insert("fu-1".to_string(), "de-1".to_string());

        // Ephemeral (RUN path): app_state projects the ephemeral enum int.
        let eph = build_app_publish_request(
            "ap-1",
            "my-app",
            AppState::Ephemeral,
            function_ids.clone(),
            definition_ids.clone(),
        );
        assert_eq!(eph.app_id, "ap-1");
        assert_eq!(eph.name, "my-app");
        assert_eq!(eph.app_state, AppState::Ephemeral as i32);
        // The maps round-trip unchanged.
        assert_eq!(
            eph.function_ids.get("handler").map(String::as_str),
            Some("fu-1")
        );
        assert_eq!(
            eph.definition_ids.get("fu-1").map(String::as_str),
            Some("de-1")
        );

        // Deployed (DEPLOY path): same maps, the deployed enum int.
        let dep = build_app_publish_request(
            "ap-1",
            "my-app",
            AppState::Deployed,
            function_ids,
            definition_ids,
        );
        assert_eq!(dep.app_state, AppState::Deployed as i32);
        assert_ne!(
            dep.app_state, eph.app_state,
            "deployed vs ephemeral project DISTINCT enum ints"
        );
    }
}
