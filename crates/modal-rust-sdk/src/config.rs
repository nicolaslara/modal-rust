//! Modal credential / endpoint configuration.
//!
//! [`ModalConfig`] is the resolved, validated configuration the client connects
//! with. [`read_modal_config`] reproduces the Python SDK precedence: a TOML file
//! (`~/.modal.toml`, or `MODAL_CONFIG_PATH`) selected by profile, then env-var
//! overrides applied last. Unlike modal-rs, a complete `MODAL_TOKEN_ID` +
//! `MODAL_TOKEN_SECRET` pair in the environment lets us connect even when no
//! config file exists (CI / container friendliness; spec §4.1.6).

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::error::{Error, Result};

/// Default Modal control-plane endpoint.
pub const DEFAULT_SERVER_URL: &str = "https://api.modal.com";
/// Default environment name used when neither config nor env specifies one.
pub const DEFAULT_ENVIRONMENT: &str = "main";

/// Resolved Modal configuration used to construct a [`crate::ModalClient`].
#[derive(Debug, Clone)]
pub struct ModalConfig {
    /// Selected profile name (the TOML table name, or a synthetic label when
    /// the config came purely from env vars / explicit credentials).
    pub profile: String,
    /// Modal server URL (e.g. `https://api.modal.com`). Always non-empty.
    pub server_url: String,
    /// Modal token id (required, validated non-empty).
    pub token_id: String,
    /// Modal token secret (required, validated non-empty).
    pub token_secret: String,
    /// Default environment for ops; call sites fall back to
    /// [`DEFAULT_ENVIRONMENT`] when this is `None`.
    pub environment: Option<String>,
    /// Image builder version (if present in config/env).
    pub image_builder_version: Option<String>,
}

impl ModalConfig {
    /// Build a config from explicit credentials, defaulting the endpoint to
    /// [`DEFAULT_SERVER_URL`] and bypassing any config file. Validates tokens.
    pub fn from_credentials(token_id: &str, token_secret: &str) -> Result<Self> {
        let cfg = Self {
            profile: "runtime".to_string(),
            server_url: DEFAULT_SERVER_URL.to_string(),
            token_id: token_id.trim().to_string(),
            token_secret: token_secret.trim().to_string(),
            environment: None,
            image_builder_version: None,
        };
        cfg.validate()?;
        Ok(cfg)
    }

    /// The environment to use for ops, defaulting to [`DEFAULT_ENVIRONMENT`].
    pub fn environment_or_default(&self) -> &str {
        self.environment.as_deref().unwrap_or(DEFAULT_ENVIRONMENT)
    }

    /// Reject empty/whitespace credentials (spec §4.1.5).
    fn validate(&self) -> Result<()> {
        if self.token_id.trim().is_empty() {
            return Err(Error::config("Missing token_id (Modal credentials)"));
        }
        if self.token_secret.trim().is_empty() {
            return Err(Error::config("Missing token_secret (Modal credentials)"));
        }
        Ok(())
    }
}

/// A single profile entry in `~/.modal.toml`. snake_case is canonical; the
/// camelCase serde aliases mirror modal-rs as a courtesy for files written by
/// other SDKs.
#[derive(Deserialize, Debug, Clone, Default)]
pub struct ModalProfile {
    /// Token id.
    #[serde(alias = "tokenId")]
    pub token_id: Option<String>,
    /// Token secret.
    #[serde(alias = "tokenSecret")]
    pub token_secret: Option<String>,
    /// Server URL override.
    #[serde(alias = "serverUrl")]
    pub server_url: Option<String>,
    /// Default environment.
    pub environment: Option<String>,
    /// Image builder version.
    #[serde(alias = "imageBuilderVersion")]
    pub image_builder_version: Option<String>,
    /// Profile selector: the first profile with `active = true` is chosen when
    /// `MODAL_PROFILE` is unset.
    pub active: Option<bool>,
}

/// Read and resolve Modal configuration (spec §4.1).
///
/// Resolution order: config file (path from `MODAL_CONFIG_PATH` or
/// `$HOME/.modal.toml`) → profile selection (`MODAL_PROFILE`, else first
/// `active = true`, else first table) → env-var overrides (win over file). If
/// the file is absent but `MODAL_TOKEN_ID` + `MODAL_TOKEN_SECRET` are both set,
/// the file is treated as optional.
pub fn read_modal_config() -> Result<ModalConfig> {
    let path = resolve_config_path();

    // Start from the file when present; otherwise an empty profile that env vars
    // can fully populate (CI/container case).
    let (profile_name, profile) = match std::fs::read_to_string(&path) {
        Ok(content) => {
            let profiles = parse_profiles(&content)?;
            if profiles.is_empty() {
                return Err(Error::config(format!(
                    "no profiles found in {}",
                    path.display()
                )));
            }
            let requested = env_string("MODAL_PROFILE");
            let selected = select_profile_name(&profiles, requested.as_deref())?;
            let profile = profiles
                .iter()
                .find(|(name, _)| name == &selected)
                .map(|(_, p)| p.clone())
                .unwrap_or_default();
            (selected, profile)
        }
        Err(file_err) => {
            // File missing/unreadable is only acceptable when both env tokens are present.
            if env_string("MODAL_TOKEN_ID").is_some() && env_string("MODAL_TOKEN_SECRET").is_some()
            {
                ("env".to_string(), ModalProfile::default())
            } else {
                return Err(Error::config(format!(
                    "failed to read Modal config at {} ({file_err}); set MODAL_TOKEN_ID + \
                     MODAL_TOKEN_SECRET to connect without a config file",
                    path.display()
                )));
            }
        }
    };

    let mut resolved = ModalConfig {
        profile: profile_name,
        server_url: trimmed_or(profile.server_url.as_deref(), DEFAULT_SERVER_URL),
        token_id: trimmed(profile.token_id.as_deref()).unwrap_or_default(),
        token_secret: trimmed(profile.token_secret.as_deref()).unwrap_or_default(),
        environment: trimmed(profile.environment.as_deref()),
        image_builder_version: trimmed(profile.image_builder_version.as_deref()),
    };

    apply_env_overrides(&mut resolved);
    resolved.validate()?;
    Ok(resolved)
}

fn resolve_config_path() -> PathBuf {
    if let Some(path) = env_string("MODAL_CONFIG_PATH") {
        return PathBuf::from(path);
    }
    let home = std::env::var("HOME").unwrap_or_default();
    Path::new(&home).join(".modal.toml")
}

fn parse_profiles(content: &str) -> Result<Vec<(String, ModalProfile)>> {
    let value: toml::Value = toml::from_str(content)?;
    let table = value
        .as_table()
        .ok_or_else(|| Error::config("Modal config must be a top-level TOML table"))?;

    let mut profiles = Vec::new();
    for (name, profile_value) in table.iter() {
        let profile: ModalProfile = profile_value
            .clone()
            .try_into()
            .map_err(|err| Error::config(format!("failed to parse profile '{name}': {err}")))?;
        profiles.push((name.to_string(), profile));
    }
    Ok(profiles)
}

fn select_profile_name(
    profiles: &[(String, ModalProfile)],
    requested: Option<&str>,
) -> Result<String> {
    if let Some(requested) = requested {
        if profiles.iter().any(|(name, _)| name == requested) {
            return Ok(requested.to_string());
        }
        return Err(Error::config(format!(
            "MODAL_PROFILE '{requested}' not found in config"
        )));
    }

    if let Some((name, _)) = profiles.iter().find(|(_, p)| p.active.unwrap_or(false)) {
        return Ok(name.clone());
    }

    Ok(profiles
        .first()
        .map(|(name, _)| name.clone())
        .unwrap_or_else(|| "default".to_string()))
}

fn apply_env_overrides(config: &mut ModalConfig) {
    if let Some(v) = env_string("MODAL_SERVER_URL") {
        config.server_url = v;
    }
    if let Some(v) = env_string("MODAL_TOKEN_ID") {
        config.token_id = v;
    }
    if let Some(v) = env_string("MODAL_TOKEN_SECRET") {
        config.token_secret = v;
    }
    if let Some(v) = env_string("MODAL_ENVIRONMENT") {
        config.environment = Some(v);
    }
    if let Some(v) = env_string("MODAL_IMAGE_BUILDER_VERSION") {
        config.image_builder_version = Some(v);
    }
}

/// Read an env var, treating empty/whitespace as unset.
fn env_string(key: &str) -> Option<String> {
    match std::env::var(key) {
        Ok(value) => trimmed(Some(value.as_str())),
        Err(_) => None,
    }
}

fn trimmed(value: Option<&str>) -> Option<String> {
    value.and_then(|v| {
        let t = v.trim();
        if t.is_empty() {
            None
        } else {
            Some(t.to_string())
        }
    })
}

fn trimmed_or(value: Option<&str>, default: &str) -> String {
    trimmed(value).unwrap_or_else(|| default.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selects_requested_profile() {
        let content = "\
[default]
token_id = \"id1\"
token_secret = \"secret1\"

[other]
token_id = \"id2\"
token_secret = \"secret2\"
";
        let profiles = parse_profiles(content).expect("parse");
        assert_eq!(
            select_profile_name(&profiles, Some("other")).expect("select"),
            "other"
        );
        assert!(select_profile_name(&profiles, Some("missing")).is_err());
    }

    #[test]
    fn selects_first_active_profile_when_unset() {
        let content = "\
[alpha]
token_id = \"id1\"
token_secret = \"secret1\"

[beta]
token_id = \"id2\"
token_secret = \"secret2\"
active = true
";
        let profiles = parse_profiles(content).expect("parse");
        assert_eq!(
            select_profile_name(&profiles, None).expect("select"),
            "beta"
        );
    }

    #[test]
    fn camelcase_aliases_parse() {
        let content = "\
[default]
tokenId = \"id1\"
tokenSecret = \"secret1\"
serverUrl = \"https://example.test\"
";
        let profiles = parse_profiles(content).expect("parse");
        let (_, profile) = &profiles[0];
        assert_eq!(profile.token_id.as_deref(), Some("id1"));
        assert_eq!(profile.token_secret.as_deref(), Some("secret1"));
        assert_eq!(profile.server_url.as_deref(), Some("https://example.test"));
    }

    #[test]
    fn from_credentials_validates_and_defaults() {
        let cfg = ModalConfig::from_credentials("ak-1", "as-1").expect("creds");
        assert_eq!(cfg.server_url, DEFAULT_SERVER_URL);
        assert_eq!(cfg.environment_or_default(), DEFAULT_ENVIRONMENT);
        assert!(ModalConfig::from_credentials("  ", "secret").is_err());
        assert!(ModalConfig::from_credentials("id", "   ").is_err());
    }

    #[test]
    fn validate_rejects_blank_tokens() {
        let cfg = ModalConfig {
            profile: "p".into(),
            server_url: DEFAULT_SERVER_URL.into(),
            token_id: "  ".into(),
            token_secret: "secret".into(),
            environment: None,
            image_builder_version: None,
        };
        assert!(cfg.validate().is_err());
    }
}
