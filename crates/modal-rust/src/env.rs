//! The `MODAL_RUST_*` environment-variable registry (M4).
//!
//! Every `MODAL_RUST_*` name the project reads is declared here ONCE, as a const.
//! These names cross three trust boundaries — local process → baked image `ENV` →
//! container Python — so a typo in any one copy silently disables a feature. Rust
//! call sites must spell names via these consts; the Python wrappers keep their own
//! literals (they are baked into deployed images and cannot import this crate), and
//! this module's drift-guard test is what ties the two sides together: every
//! `MODAL_RUST_*` literal anywhere under `crates/*/src` (including
//! `remote/wrapper.py` / `deploy/wrapper.py`) and `crates/modal-rust/tests` must be
//! declared below.
//!
//! NEVER rename a variable: deployed images bake these names into `ENV` layers, so
//! a rename strands every already-deployed app.
//!
//! The user-facing table (purpose, where read, public vs internal) lives in
//! README.md § "Environment variables".

/// Cargo package built/invoked remotely (`cargo build -p <pkg>`). Overrides the
/// macro-detected `CARGO_PKG_NAME`. Read by `RemoteConfig::default()` /
/// `App::connect`.
pub const PACKAGE: &str = "MODAL_RUST_PACKAGE";

/// Source dir to upload; default: nearest ancestor `[workspace]` `Cargo.toml`
/// (else nearest `Cargo.toml`, else CWD). Read by `RemoteConfig::default()`.
pub const SOURCE_DIR: &str = "MODAL_RUST_SOURCE_DIR";

/// Base image for the run/deploy build (default `rust:<ver>-slim`). Pairs with
/// [`INSTALL_RUST`] for an env-driven CUDA path. Read by `RemoteConfig::default()`.
pub const BASE_IMAGE: &str = "MODAL_RUST_BASE_IMAGE";

/// Truthy ⇒ install the Rust toolchain into the image (for bases without one,
/// e.g. CUDA-devel). Read by `RemoteConfig::default()`.
pub const INSTALL_RUST: &str = "MODAL_RUST_INSTALL_RUST";

/// Truthy ⇒ disable the run-path cargo build cache (default ON). Read by
/// `RemoteConfig::default()`.
pub const NO_CACHE: &str = "MODAL_RUST_NO_CACHE";

/// `target/` archiving in the build cache — DEFAULT ON; set `0`/`false`/`no`/
/// `off` to opt OUT (without `target/`, every fresh container recompiles the
/// whole dep graph). Read locally by `discover_cache_target`; an opt-out is
/// baked as `=0` into the run image `ENV` so the container wrapper (also
/// default-ON) honors it.
pub const CACHE_TARGET: &str = "MODAL_RUST_CACHE_TARGET";

/// Stable deploy app name override (default `DEFAULT_DEPLOY_APP`). Read by
/// `DeployConfig::default()`.
pub const DEPLOY_APP: &str = "MODAL_RUST_DEPLOY_APP";

/// Truthy ⇒ a failed snapshot prime degrades to lazy `#[enter]` instead of
/// failing container init. Read at deploy time by `DeployConfig::for_app` and
/// baked into the image `ENV` for the deploy wrapper.
pub const SNAPSHOT_BEST_EFFORT: &str = "MODAL_RUST_SNAPSHOT_BEST_EFFORT";

/// INTERNAL (container-side escape hatch): set to a falsy value to make the
/// wrappers print the build/exec commands instead of running them. Read only by
/// the Python wrappers.
pub const SERVE: &str = "MODAL_RUST_SERVE";

/// INTERNAL: baked into the deploy image `ENV` when any entrypoint enables
/// `enable_memory_snapshot`; gates the deploy wrapper's import-time prime.
pub const SNAPSHOT_PRIME: &str = "MODAL_RUST_SNAPSHOT_PRIME";

/// INTERNAL: base64-encoded JSON run config the facade bakes into the run image
/// `ENV` for `remote/wrapper.py` (see `remote::WRAPPER_CONFIG_ENV`).
pub const RUN_CONFIG_JSON_B64: &str = "MODAL_RUST_RUN_CONFIG_JSON_B64";

/// TEST-ONLY: the secret key the live secrets/volumes test round-trips.
pub const TEST_SECRET: &str = "MODAL_RUST_TEST_SECRET";

/// TEST-ONLY: overrides the deploy wrapper's `/app/modal_runner` path so
/// `deploy/wrapper_test.py` can exec a stub runner. Read only by the wrapper.
pub const RUNNER: &str = "MODAL_RUST_RUNNER";

/// The ONE truthy parser for `MODAL_RUST_*` boolean knobs: trimmed,
/// case-insensitive `1`/`true`/`yes`/`on` ⇒ `true`; anything else (including
/// unset) ⇒ `false`. Callers wanting "default ON" negate an inverted knob (e.g.
/// `!env_bool(NO_CACHE)`). Rust-side only — [`SERVE`]'s Python-side default-ON
/// falsy check stays in the wrappers.
pub fn env_bool(name: &str) -> bool {
    std::env::var(name)
        .map(|v| {
            matches!(
                v.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    /// Every name this module declares. The drift guard checks found literals
    /// against THIS list, so a new variable must be added here (with docs) first.
    const DECLARED: &[&str] = &[
        super::PACKAGE,
        super::SOURCE_DIR,
        super::BASE_IMAGE,
        super::INSTALL_RUST,
        super::NO_CACHE,
        super::CACHE_TARGET,
        super::DEPLOY_APP,
        super::SNAPSHOT_BEST_EFFORT,
        super::SERVE,
        super::SNAPSHOT_PRIME,
        super::RUN_CONFIG_JSON_B64,
        super::TEST_SECRET,
        super::RUNNER,
    ];

    /// Collect `.rs` / `.py` files recursively (skips `target/` just in case).
    fn collect_sources(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if path.file_name().is_some_and(|n| n == "target") {
                    continue;
                }
                collect_sources(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs" || e == "py") {
                out.push(path);
            }
        }
    }

    /// Extract every `MODAL_RUST_[A-Z0-9_]+` token from `text`, skipping matches
    /// preceded by `_` (the macros' `__MODAL_RUST_CLS_*` symbol prefix is a Rust
    /// identifier namespace, not an env var).
    fn extract_tokens(text: &str) -> Vec<String> {
        const PREFIX: &str = "MODAL_RUST_";
        let bytes = text.as_bytes();
        let mut tokens = Vec::new();
        let mut from = 0;
        while let Some(pos) = text[from..].find(PREFIX) {
            let start = from + pos;
            let mut end = start + PREFIX.len();
            while end < bytes.len()
                && (bytes[end].is_ascii_uppercase()
                    || bytes[end].is_ascii_digit()
                    || bytes[end] == b'_')
            {
                end += 1;
            }
            // Skip `__MODAL_RUST_CLS_*` style identifiers and bare `MODAL_RUST_`
            // prefix mentions (e.g. doc text "MODAL_RUST_*").
            let preceded_by_underscore = start > 0 && bytes[start - 1] == b'_';
            if !preceded_by_underscore && end > start + PREFIX.len() {
                tokens.push(text[start..end].trim_end_matches('_').to_string());
            }
            from = start + PREFIX.len();
        }
        tokens
    }

    /// DRIFT GUARD (M4): every `MODAL_RUST_*` literal in `crates/*/src` (Rust AND
    /// the Python wrappers/wrapper-tests) and `crates/modal-rust/tests` must be a
    /// name declared in this module — or a `_`-boundary prefix of one (tests
    /// intentionally assert with `contains("MODAL_RUST_SNAPSHOT")`). This is the
    /// link that keeps the wrappers' own Python literals honest without renaming
    /// anything baked into deployed images.
    #[test]
    fn every_modal_rust_literal_is_declared() {
        let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let crates_root = manifest.parent().expect("crates/ root");
        let mut files = Vec::new();
        let crate_dirs = std::fs::read_dir(crates_root).expect("read crates/");
        for entry in crate_dirs.flatten() {
            collect_sources(&entry.path().join("src"), &mut files);
        }
        collect_sources(&manifest.join("tests"), &mut files);
        assert!(
            files.iter().any(|f| f.ends_with("deploy/wrapper.py")),
            "scan must cover the Python wrappers; got {} files",
            files.len()
        );

        let mut undeclared = Vec::new();
        for file in &files {
            let text = std::fs::read_to_string(file)
                .unwrap_or_else(|e| panic!("read {}: {e}", file.display()));
            for token in extract_tokens(&text) {
                let declared = DECLARED.contains(&token.as_str())
                    || DECLARED.iter().any(|d| d.starts_with(&format!("{token}_")));
                if !declared {
                    undeclared.push(format!("{} ({})", token, file.display()));
                }
            }
        }
        assert!(
            undeclared.is_empty(),
            "MODAL_RUST_* literals not declared in modal_rust::env \
             (add the const + README table row): {undeclared:?}"
        );
    }

    /// `env_bool` truthy table, via a probe name OUTSIDE the `MODAL_RUST_` prefix
    /// so no real knob is disturbed and the drift guard above stays quiet.
    /// Serialized: env vars are process-global (see `crate::ENV_TEST_LOCK`).
    #[test]
    fn env_bool_truthy_table() {
        let _guard = crate::ENV_TEST_LOCK.lock().unwrap();
        const VAR: &str = "ENV_BOOL_PROBE_FOR_MODAL_RUST_TEST";
        std::env::remove_var(VAR);
        assert!(!super::env_bool(VAR), "unset => false");
        for truthy in ["1", "true", "YES", " on ", "True"] {
            std::env::set_var(VAR, truthy);
            assert!(super::env_bool(VAR), "{truthy:?} must parse truthy");
        }
        for falsy in ["0", "false", "no", "off", "", "2"] {
            std::env::set_var(VAR, falsy);
            assert!(!super::env_bool(VAR), "{falsy:?} must parse falsy");
        }
        std::env::remove_var(VAR);
    }
}
