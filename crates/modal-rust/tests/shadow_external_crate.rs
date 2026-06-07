//! Fix (user-generality): the local `--describe` SHADOW build must work for an EXTERNAL
//! standalone crate that deps `modal-rust` by a RELATIVE out-of-workspace path — the
//! natural local-dev layout, since modal-rust is not on crates.io.
//!
//! The bug: `materialize_shadow` copied the crate's `Cargo.toml` verbatim into a temp
//! shadow dir; cargo then resolved the relative `../checkout/...` modal-rust path-dep
//! against the TEMP shadow root (where it does not exist) and failed with
//! `failed to read .../Cargo.toml: No such file or directory`. The fix rewrites every
//! relative out-of-closure path-dep to an absolute path anchored at the REAL on-disk
//! crate dir, so the shadow resolves modal-rust identically to the real tree.
//!
//! This test is FAST: it materializes the shadow and proves cargo can RESOLVE the shadow
//! manifest (`cargo metadata` reproduces the exact original failure mode) without running
//! the full release build. The full build is covered by the live/example suites.

use std::fs;
use std::path::Path;

/// The repo root (this test crate is `crates/modal-rust`).
fn repo_root() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("repo root is two levels above crates/modal-rust")
        .to_path_buf()
}

#[test]
fn shadow_resolves_external_crate_with_relative_out_of_workspace_facade_dep() {
    let facade_dir = repo_root().join("crates/modal-rust");

    // An external standalone crate `myapp` OUTSIDE the repo workspace that deps the
    // facade by a RELATIVE path. `myapp` is its own ws root + sole member, so the facade
    // path-dep escapes the workspace (the headline external case).
    let tmp = std::env::temp_dir().join(format!("mr-shadow-ext-{}", std::process::id()));
    fs::create_dir_all(&tmp).unwrap();
    // Canonicalize the temp root so a symlinked temp dir (macOS `/tmp` -> `/private/tmp`)
    // does not skew the genuinely-relative `..` path we declare below.
    let tmp = fs::canonicalize(&tmp).unwrap();
    let app_dir = tmp.join("myapp");
    fs::create_dir_all(app_dir.join("src")).unwrap();

    // Relative path from the app dir to the on-disk facade.
    let rel = pathdiff_relative(&fs::canonicalize(&facade_dir).unwrap(), &app_dir);
    let rel_posix = rel.to_string_lossy().replace('\\', "/");

    fs::write(
        app_dir.join("Cargo.toml"),
        format!(
            "[package]\nname = \"myapp\"\nversion = \"0.0.0\"\nedition = \"2021\"\n\n\
             [lib]\nname = \"myapp\"\npath = \"src/lib.rs\"\n\n\
             [dependencies]\nmodal-rust = {{ path = \"{rel_posix}\" }}\n\
             serde = {{ version = \"1\", features = [\"derive\"] }}\n"
        ),
    )
    .unwrap();
    fs::write(
        app_dir.join("src/lib.rs"),
        "use serde::{Deserialize, Serialize};\n\
         #[derive(Serialize, Deserialize)]\npub struct In { pub a: i64, pub b: i64 }\n\
         #[derive(Serialize, Deserialize)]\npub struct Out { pub sum: i64 }\n\
         #[modal_rust::function]\n\
         pub fn add(i: In) -> Result<Out, std::convert::Infallible> { Ok(Out { sum: i.a + i.b }) }\n",
    )
    .unwrap();

    // Resolve + materialize the shadow exactly as the CLI `--describe` path does.
    let target =
        modal_rust::resolve_runner_target(&app_dir, "myapp").expect("runner target resolves");
    assert!(
        target.is_generatable(),
        "no own bin + facade dep => the tooling generates the runner"
    );
    let dest = tmp.join("_shadow");
    let _ = fs::remove_dir_all(&dest);
    modal_rust::materialize_shadow(&app_dir, &target, &dest).expect("materialize shadow");

    // The shadow root manifest's relative facade path-dep is now ABSOLUTE (anchored at
    // the real on-disk facade), so it resolves regardless of the shadow's location.
    let shadow_manifest = fs::read_to_string(dest.join("Cargo.toml")).unwrap();
    let facade_abs = fs::canonicalize(&facade_dir).unwrap();
    assert!(
        shadow_manifest.contains(&facade_abs.to_string_lossy().to_string()),
        "shadow manifest pins the absolute facade path; got:\n{shadow_manifest}"
    );
    assert!(
        !shadow_manifest.contains(&rel_posix),
        "the relative out-of-workspace spec was rewritten away; got:\n{shadow_manifest}"
    );

    // The generated runner bin landed in the shadow.
    assert!(
        dest.join("src/bin/modal_runner.rs").is_file(),
        "the generated modal_runner bin is materialized in the shadow"
    );

    // The conclusive check: cargo can RESOLVE the shadow manifest. Before the fix this
    // failed with "failed to read .../Cargo.toml: No such file or directory".
    let out =
        std::process::Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
            .args([
                "metadata",
                "--format-version",
                "1",
                "--no-deps",
                "--manifest-path",
            ])
            .arg(dest.join("Cargo.toml"))
            .output()
            .expect("spawn cargo metadata");
    assert!(
        out.status.success(),
        "cargo metadata must resolve the shadow manifest; stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let _ = fs::remove_dir_all(&tmp);
}

/// Minimal relative-path computation (`pathdiff`-style) from `base` ancestor logic:
/// the path of `target` relative to `from`. Both must be absolute. Used so the test
/// declares a genuinely RELATIVE path-dep (the bug only reproduces for relative specs).
fn pathdiff_relative(target: &Path, from: &Path) -> std::path::PathBuf {
    use std::path::Component;
    let mut ta = target.components().peekable();
    let mut fa = from.components().peekable();
    // Drop the common prefix.
    while let (Some(t), Some(f)) = (ta.peek(), fa.peek()) {
        if t == f {
            ta.next();
            fa.next();
        } else {
            break;
        }
    }
    let mut result = std::path::PathBuf::new();
    for c in fa {
        if matches!(c, Component::Normal(_)) {
            result.push("..");
        }
    }
    for c in ta {
        result.push(c.as_os_str());
    }
    result
}
