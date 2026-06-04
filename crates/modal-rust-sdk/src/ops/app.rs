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
        let resp = self
            .inner_mut()
            .app_get_or_create(AppGetOrCreateRequest {
                app_name: app_name.to_string(),
                environment_name,
                object_creation_type: ObjectCreationType::CreateIfMissing as i32,
            })
            .await?
            .into_inner();
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
        let resp = self
            .inner_mut()
            .app_create(AppCreateRequest {
                client_id: String::new(),
                description: description.to_string(),
                environment_name,
                app_state: AppState::Ephemeral as i32,
                tags: HashMap::new(),
            })
            .await?
            .into_inner();
        Ok(resp.app_id)
    }

    /// Deploy via `AppPublish` (api.proto:4147) ONLY — **fix #2**.
    ///
    /// - `function_ids`: `function_name` → `function_id`.
    /// - `definition_ids`: `function_id` → `definition_id`
    ///   (from `FunctionCreateResponse.handle_metadata.definition_id`).
    ///
    /// Publishes the app into `APP_STATE_DEPLOYED` so `FunctionGet`/`from_name`
    /// resolves it. We never issue the legacy `AppSetObjects` RPC.
    pub async fn app_publish(
        &mut self,
        app_id: &str,
        app_name: &str,
        function_ids: HashMap<String, String>,
        definition_ids: HashMap<String, String>,
    ) -> Result<PublishedApp> {
        let resp = self
            .inner_mut()
            .app_publish(AppPublishRequest {
                app_id: app_id.to_string(),
                name: app_name.to_string(),
                app_state: AppState::Deployed as i32,
                function_ids,
                definition_ids,
                ..Default::default()
            })
            .await?
            .into_inner();

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
}
