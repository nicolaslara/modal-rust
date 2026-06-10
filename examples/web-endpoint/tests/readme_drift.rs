//! README DRIFT GUARD: the `#[endpoint]` code blocks shown in THIS crate's README.md
//! AND in the root README's web-endpoints section MUST equal this crate's real,
//! compiled source. A stale README is a TEST FAILURE (not a silent doc rot) —
//! mirroring the quickstart / stateful-class / snapshot-class guards.
//!
//! Mechanism: both READMEs tag the authored block as ```` ```rust endpoint ```` and
//! this crate's `src/lib.rs` brackets the same region (the `Summary` struct through the
//! end of the `#[endpoint]` fn) with `// endpoint:begin` / `// endpoint:end` markers.
//! The test extracts both and asserts byte-equality (after trimming surrounding blank
//! lines). Because the crate compiles + its tests pass, "the README equals the crate
//! source" implies "the README compiles + works".

use std::path::{Path, PathBuf};

/// Repo root = two levels up from this crate (`examples/web-endpoint`).
fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
}

/// Extract the code between `// endpoint:begin` and `// endpoint:end` (exclusive of
/// the marker lines) from this crate's `src/lib.rs`. The begin marker is on its own
/// line, so we cut at the first newline AFTER it.
fn crate_endpoint_block() -> String {
    let lib = std::fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("src")
            .join("lib.rs"),
    )
    .expect("read web-endpoint/src/lib.rs");
    let begin = "// endpoint:begin";
    let end = "// endpoint:end";
    let after_marker = lib
        .split_once(begin)
        .unwrap_or_else(|| panic!("missing {begin:?} marker in src/lib.rs"))
        .1;
    // Skip to the end of the begin-marker line, then take up to the end-marker line.
    let after_marker_line = after_marker
        .split_once('\n')
        .expect("endpoint:begin marker line must end with a newline")
        .1;
    let body = after_marker_line
        .split_once(end)
        .unwrap_or_else(|| panic!("missing {end:?} marker in src/lib.rs"))
        .0;
    body.trim_matches('\n').to_string()
}

/// Extract the ```` ```rust endpoint ```` fenced block from the README at `path`.
fn readme_endpoint_block(path: &Path) -> String {
    let readme =
        std::fs::read_to_string(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    // Allow either ```rust endpoint or ```rust,endpoint.
    let open_a = "```rust endpoint";
    let open_b = "```rust,endpoint";
    let (idx, open) = readme
        .find(open_a)
        .map(|i| (i, open_a))
        .or_else(|| readme.find(open_b).map(|i| (i, open_b)))
        .unwrap_or_else(|| {
            panic!(
                "{} must contain a ```rust endpoint code block (the drift-guarded \
                 endpoint surface). Tag the #[endpoint] fence exactly ```rust endpoint.",
                path.display()
            )
        });
    let after_fence = &readme[idx + open.len()..];
    // Skip to the end of the fence line, then take up to the closing ```.
    let after_nl = after_fence
        .split_once('\n')
        .expect("fence line must end with a newline")
        .1;
    let body = after_nl
        .split_once("\n```")
        .expect("README endpoint block must be closed with ```")
        .0;
    body.trim_matches('\n').to_string()
}

#[test]
fn example_readme_endpoint_matches_crate_source() {
    let from_crate = crate_endpoint_block();
    let readme = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("README.md");
    let from_readme = readme_endpoint_block(&readme);
    assert_eq!(
        from_readme, from_crate,
        "\n\nexamples/web-endpoint/README.md `#[endpoint]` block has DRIFTED from \
         src/lib.rs.\nThe README is a tested artifact: update the ```rust endpoint \
         block to match the crate (or vice-versa).\n\n--- README ---\n{from_readme}\n\n\
         --- CRATE ---\n{from_crate}\n"
    );
}

#[test]
fn root_readme_endpoint_matches_crate_source() {
    let from_crate = crate_endpoint_block();
    let from_readme = readme_endpoint_block(&repo_root().join("README.md"));
    assert_eq!(
        from_readme, from_crate,
        "\n\nroot README.md `#[endpoint]` block has DRIFTED from \
         examples/web-endpoint/src/lib.rs.\nThe README is a tested artifact: update \
         the ```rust endpoint block to match the crate (or vice-versa).\n\n\
         --- README ---\n{from_readme}\n\n--- CRATE ---\n{from_crate}\n"
    );
}
