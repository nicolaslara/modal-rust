//! Secret resolution — `SecretGetOrCreate`.
//!
//! Two paths, both over the SAME RPC (`SecretGetOrCreate`, api.proto:4327):
//!
//! - [`ModalClient::secret_get_or_create`] — look up a deployed secret BY NAME and
//!   return its `secret_id`, for attaching to a function via
//!   `FunctionSpec.secret_ids` -> `Function.secret_ids`. Mirrors
//!   `Secret.from_name` (secret.py:402): sets `deployment_name` +
//!   `environment_name` (+ optional `required_keys`); the server asserts the
//!   required keys exist. This is the USER-facing `#[function(secrets = [..])]`
//!   path.
//!
//! - [`ModalClient::secret_from_dict`] — CREATE (idempotently) a named secret from
//!   a `{key: value}` map and return its `secret_id`. Used by tests / ephemeral
//!   setup so a live proof can provision its own secret with no manual `modal
//!   secret create`. Mirrors `Secret.from_dict` + `_create_deployed` (secret.py):
//!   sends `deployment_name` + `env_dict` + `object_creation_type =
//!   CREATE_IF_MISSING`, which is idempotent + retry-safe.
//!
//! The key/values of an attached secret are injected as ENV VARS in the container
//! by Modal — readable by the user fn (`std::env`) and the runner. We NEVER log the
//! values (Modal/secrets rules).

use std::collections::HashMap;

use crate::client::ModalClient;
use crate::error::{Error, Result};
use crate::proto::api::{ObjectCreationType, SecretGetOrCreateRequest};

/// Build the `SecretGetOrCreate` request for the `from_name` PURE-LOOKUP path —
/// pure, no I/O. Mirrors `Secret.from_name`: `object_creation_type` UNSPECIFIED, no
/// `env_dict`; the optional `required_keys` ride through (the server asserts them).
///
/// Extracted from [`ModalClient::secret_get_or_create`]; the method passes the
/// resolved `environment_name`.
pub(crate) fn build_secret_from_name_request(
    name: &str,
    required_keys: &[String],
    environment_name: String,
) -> SecretGetOrCreateRequest {
    SecretGetOrCreateRequest {
        deployment_name: name.to_string(),
        environment_name,
        object_creation_type: ObjectCreationType::Unspecified as i32,
        required_keys: required_keys.to_vec(),
        ..Default::default() // env_dict empty; app_id empty; reserved namespace unset
    }
}

/// Build the `SecretGetOrCreate` request for the `from_dict` CREATE path — pure, no
/// I/O. Mirrors `Secret.from_dict`/`_create_deployed`: `object_creation_type`
/// CREATE_IF_MISSING (idempotent) + the `env_dict`; no `required_keys`. The VALUES
/// are never logged.
///
/// Extracted from [`ModalClient::secret_from_dict`]; the method passes the resolved
/// `environment_name`.
pub(crate) fn build_secret_from_dict_request(
    name: &str,
    env: &HashMap<String, String>,
    environment_name: String,
) -> SecretGetOrCreateRequest {
    SecretGetOrCreateRequest {
        deployment_name: name.to_string(),
        environment_name,
        object_creation_type: ObjectCreationType::CreateIfMissing as i32,
        env_dict: env.clone(),
        ..Default::default() // required_keys empty; app_id empty
    }
}

impl ModalClient {
    /// Resolve a deployed Secret BY NAME and return its `secret_id`. Mirrors
    /// `Secret.from_name` (secret.py:402): a pure lookup (`object_creation_type`
    /// UNSPECIFIED). When `required_keys` is non-empty the server asserts those keys
    /// exist on the secret (and errors if not).
    ///
    /// - `name`: secret deployment name (e.g. `"my-secret"`).
    /// - `required_keys`: optional asserted-present keys (empty = no assertion).
    /// - `environment`: defaults to the configured environment (or `"main"`).
    pub async fn secret_get_or_create(
        &mut self,
        name: &str,
        required_keys: &[String],
        environment: Option<&str>,
    ) -> Result<String> {
        let environment_name = self.env_or_default(environment);
        // Pure lookup (UNSPECIFIED == "just lookup", api.proto:208) — mirrors
        // `Secret.from_name`, which sets neither object_creation_type nor env_dict.
        let req = build_secret_from_name_request(name, required_keys, environment_name);
        self.secret_get_or_create_inner("secret_get_or_create", req, name)
            .await
    }

    /// CREATE (idempotently) a named Secret from a `{key: value}` env map and return
    /// its `secret_id`. Mirrors `Secret.from_dict`/`_create_deployed`: sends
    /// `deployment_name` + `env_dict` + `object_creation_type = CREATE_IF_MISSING`.
    ///
    /// CREATE_IF_MISSING is idempotent (re-running returns the SAME secret_id; if the
    /// env_dict differs from an existing secret it is ignored server-side, matching
    /// Python), so this is retry-safe and needs no manual cleanup — ideal for a live
    /// proof that provisions its own secret. The VALUES are never logged.
    ///
    /// - `name`: secret deployment name.
    /// - `env`: the key/value pairs to store (injected as ENV VARS in the container).
    /// - `environment`: defaults to the configured environment (or `"main"`).
    pub async fn secret_from_dict(
        &mut self,
        name: &str,
        env: &HashMap<String, String>,
        environment: Option<&str>,
    ) -> Result<String> {
        let environment_name = self.env_or_default(environment);
        let req = build_secret_from_dict_request(name, env, environment_name);
        self.secret_get_or_create_inner("secret_from_dict", req, name)
            .await
    }

    /// Shared `SecretGetOrCreate` RPC body: retry-wrapped unary call + empty-id
    /// guard. CREATE_IF_MISSING / pure-lookup are both idempotent server-side, so a
    /// retry after a dropped response re-resolves the same secret_id (mirrors
    /// `volume_get_or_create`).
    async fn secret_get_or_create_inner(
        &mut self,
        op: &'static str,
        req: SecretGetOrCreateRequest,
        name: &str,
    ) -> Result<String> {
        let resp = self
            .retry_rpc(op, req, |mut stub, req| async move {
                stub.secret_get_or_create(req).await
            })
            .await?;

        if resp.secret_id.is_empty() {
            return Err(Error::build(format!(
                "SecretGetOrCreate for '{name}' returned an empty secret_id"
            )));
        }
        Ok(resp.secret_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::api::ObjectCreationType;

    #[test]
    fn from_name_request_is_pure_lookup() {
        // `secret_get_or_create` (from_name) must NOT set a creation type or env_dict
        // — it is a pure lookup (UNSPECIFIED == "just lookup"). Driven THROUGH the
        // production builder (not a hand-built literal).
        let req = build_secret_from_name_request(
            "my-secret",
            &["API_KEY".to_string()],
            "main".to_string(),
        );
        assert_eq!(req.deployment_name, "my-secret");
        assert_eq!(req.environment_name, "main");
        assert_eq!(req.object_creation_type, 0);
        assert!(req.env_dict.is_empty(), "from_name sends no env_dict");
        assert_eq!(req.required_keys, vec!["API_KEY".to_string()]);
    }

    #[test]
    fn from_dict_request_is_create_if_missing() {
        // `secret_from_dict` must CREATE_IF_MISSING (idempotent) and carry the
        // env_dict; it sets no required_keys. Driven THROUGH the production builder.
        let mut env = HashMap::new();
        env.insert("FOO".to_string(), "bar".to_string());
        let req = build_secret_from_dict_request("ephemeral", &env, "main".to_string());
        assert_eq!(req.deployment_name, "ephemeral");
        assert_eq!(req.object_creation_type, 1);
        assert_eq!(req.env_dict.get("FOO").map(String::as_str), Some("bar"));
        assert!(req.required_keys.is_empty());
    }

    #[test]
    fn creation_type_constants_match_proto() {
        assert_eq!(ObjectCreationType::Unspecified as i32, 0);
        assert_eq!(ObjectCreationType::CreateIfMissing as i32, 1);
    }
}
