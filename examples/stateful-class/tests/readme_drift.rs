//! README DRIFT GUARD: the README's `#[cls]` code block MUST equal this crate's real,
//! compiled source. A stale README is a TEST FAILURE (not a silent doc rot) — mirroring
//! the quickstart guard (`examples/quickstart/tests/readme_drift.rs`).
//!
//! Mechanism: the README tags the authored `#[cls]` block as ```` ```rust cls ```` and
//! this crate's `src/lib.rs` brackets the same region (`pub struct Embedder` through the
//! end of the `#[cls] impl Embedder` block) with `// cls:begin` / `// cls:end` markers.
//! The test extracts both and asserts byte-equality (after trimming surrounding blank
//! lines). Because the crate compiles + its tests pass, "the README equals the crate
//! source" implies "the README compiles + works".

use std::path::PathBuf;

/// Repo root = two levels up from this crate (`examples/stateful-class`).
fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
}

/// Extract the code between `// cls:begin` and `// cls:end` (exclusive of the marker
/// lines) from this crate's `src/lib.rs`. The begin marker is on its own line, so we cut
/// at the first newline AFTER it.
fn crate_cls_block() -> String {
    let lib = std::fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("src")
            .join("lib.rs"),
    )
    .expect("read stateful-class/src/lib.rs");
    let begin = "// cls:begin";
    let end = "// cls:end";
    let after_marker = lib
        .split_once(begin)
        .unwrap_or_else(|| panic!("missing {begin:?} marker in src/lib.rs"))
        .1;
    // Skip to the end of the begin-marker line, then take up to the `// cls:end` line.
    let after_marker_line = after_marker
        .split_once('\n')
        .expect("cls:begin marker line must end with a newline")
        .1;
    let body = after_marker_line
        .split_once(end)
        .unwrap_or_else(|| panic!("missing {end:?} marker in src/lib.rs"))
        .0;
    body.trim_matches('\n').to_string()
}

/// Extract the ```` ```rust cls ```` fenced block from README.md.
fn readme_cls_block() -> String {
    let readme = std::fs::read_to_string(repo_root().join("README.md")).expect("read README.md");
    // Allow either ```rust cls or ```rust,cls.
    let open_a = "```rust cls";
    let open_b = "```rust,cls";
    let (idx, open) = readme
        .find(open_a)
        .map(|i| (i, open_a))
        .or_else(|| readme.find(open_b).map(|i| (i, open_b)))
        .expect(
            "README.md must contain a ```rust cls code block (the drift-guarded Cls \
             surface). Tag the #[cls] fence exactly ```rust cls.",
        );
    let after_fence = &readme[idx + open.len()..];
    // Skip to the end of the fence line, then take up to the closing ```.
    let after_nl = after_fence
        .split_once('\n')
        .expect("fence line must end with a newline")
        .1;
    let body = after_nl
        .split_once("\n```")
        .expect("README cls block must be closed with ```")
        .0;
    body.trim_matches('\n').to_string()
}

#[test]
fn readme_cls_matches_crate_source() {
    let from_crate = crate_cls_block();
    let from_readme = readme_cls_block();
    assert_eq!(
        from_readme, from_crate,
        "\n\nREADME `#[cls]` block has DRIFTED from examples/stateful-class/src/lib.rs.\n\
         The README is a tested artifact: update the ```rust cls block to match the crate \
         (or vice-versa).\n\n--- README ---\n{from_readme}\n\n--- CRATE ---\n{from_crate}\n"
    );
}
