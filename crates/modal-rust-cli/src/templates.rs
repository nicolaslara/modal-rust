//! Generated Modal Python shim templates.
//!
//! These are byte-for-byte copies of the validated prototype shims
//! (`workpads/prototype/{dev_app,deploy_app,call_app}.py`) with the **injected
//! params** replaced by `{{PLACEHOLDER}}` markers. Rendering with the prototype's
//! own param values reproduces the prototype shim exactly (modulo those injected
//! params) — the M9a byte-equivalence guard (tasks.md M9a, boundaries.md §5/§8).
//!
//! The injected params are exactly the documented normalization set: the app name,
//! the `RUST_VER` pin, the local/manifest source path, and the cargo `package` to
//! build (`-p <pkg>`, disambiguating the shared `modal_runner` bin — boundaries.md
//! §8). Per-function config (gpu/timeout/cache) is sourced dynamically from the Rust
//! `#[modal_rust::function(...)]` decorator via the facade — NOT from a CLI flag —
//! so the CLI no longer injects a `gpu=` kwarg. The entrypoint name and the input
//! are NOT baked into the shim text — they flow in at `modal run` time as
//! `--entrypoint` / `--input-json` against the `main` local_entrypoint, so the shim
//! body is parameterized by app name / rust version / source path / package.
//!
//! The CLI is a **pure wrapper**: it introduces no new Modal capability and these
//! templates must stay byte-equivalent to the prototype shims across milestones.

/// Parameters injected into the generated shims. Only these vary between the
/// generated shim and the validated prototype reference.
pub struct ShimParams {
    /// The Modal app name for the dev/run shim (prototype: `modal-rust-poc-dev`).
    pub dev_app_name: String,
    /// The persistent deploy app name (prototype: `modal-rust-add-poc`). Shared by
    /// the deploy shim (the app it registers) and the call shim (the app it looks
    /// up by name).
    pub deploy_app_name: String,
    /// The local call-shim app name (prototype: `modal-rust-call`).
    pub call_app_name: String,
    /// The pinned Rust image tag component (prototype: `1`, i.e. `rust:1-slim`).
    pub rust_ver: String,
    /// The cargo WORKSPACE ROOT mounted as `/src` (run) or `/app/src` (deploy).
    pub local_src: String,
    /// The cargo PACKAGE name (`[package].name`) the shim builds with `-p <pkg>`
    /// (prototype: `example-add`). Required because multiple workspace members
    /// share the `modal_runner` bin name, so a bare `--bin modal_runner` is
    /// ambiguous (boundaries.md §8; the run/deploy regression this fixes).
    pub package: String,
}

/// Render the `run` / dev shim (the M4 runtime-build form). Byte-equivalent to
/// `workpads/prototype/dev_app.py` when rendered with the prototype params.
pub fn dev_app(p: &ShimParams) -> String {
    DEV_APP_TEMPLATE
        .replace("{{APP_NAME}}", &p.dev_app_name)
        .replace("{{RUST_VER}}", &p.rust_ver)
        .replace("{{LOCAL_SRC}}", &p.local_src)
        .replace("{{PACKAGE}}", &p.package)
}

/// Render the `deploy` shim (build-time build, baked `/app/modal_runner`).
/// Byte-equivalent to `workpads/prototype/deploy_app.py` with prototype params.
pub fn deploy_app(p: &ShimParams) -> String {
    DEPLOY_APP_TEMPLATE
        .replace("{{APP_NAME}}", &p.deploy_app_name)
        .replace("{{RUST_VER}}", &p.rust_ver)
        .replace("{{LOCAL_SRC}}", &p.local_src)
        .replace("{{PACKAGE}}", &p.package)
}

/// Render the `call` shim (`Function.from_name(...).remote()`). Byte-equivalent to
/// `workpads/prototype/call_app.py` with prototype params.
pub fn call_app(p: &ShimParams) -> String {
    CALL_APP_TEMPLATE
        .replace("{{CALL_APP_NAME}}", &p.call_app_name)
        .replace("{{DEPLOY_APP_NAME}}", &p.deploy_app_name)
}

const DEV_APP_TEMPLATE: &str = include_str!("templates/dev_app.py.tmpl");
const DEPLOY_APP_TEMPLATE: &str = include_str!("templates/deploy_app.py.tmpl");
const CALL_APP_TEMPLATE: &str = include_str!("templates/call_app.py.tmpl");
