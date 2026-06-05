//! README DRIFT GUARD: the README's quickstart code block MUST equal this crate's
//! real, compiled source. A stale README is a TEST FAILURE (not a silent doc rot).
//!
//! Mechanism: the README tags the block as ```` ```rust quickstart ```` and this
//! crate's `src/lib.rs` brackets the same code with `// quickstart:begin` /
//! `// quickstart:end` markers. The test extracts both and asserts byte-equality
//! (after trimming surrounding blank lines). Because the crate compiles + its tests
//! pass, "the README equals the crate source" implies "the README compiles + works".

use std::path::PathBuf;

/// Repo root = two levels up from this crate (`examples/quickstart`).
fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
}

/// Extract the code between `// quickstart:begin` and `// quickstart:end` (exclusive
/// of the marker lines) from this crate's `src/lib.rs`.
fn crate_quickstart_block() -> String {
    let lib = std::fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("src")
            .join("lib.rs"),
    )
    .expect("read quickstart/src/lib.rs");
    let begin = "// quickstart:begin";
    let end = "// quickstart:end";
    let after = lib
        .split_once(begin)
        .unwrap_or_else(|| panic!("missing {begin:?} marker in src/lib.rs"))
        .1;
    let body = after
        .split_once(end)
        .unwrap_or_else(|| panic!("missing {end:?} marker in src/lib.rs"))
        .0;
    body.trim_matches('\n').to_string()
}

/// Extract the ```` ```rust quickstart ```` fenced block from README.md.
fn readme_quickstart_block() -> String {
    let readme = std::fs::read_to_string(repo_root().join("README.md")).expect("read README.md");
    // Find the opening fence (allow either ```rust quickstart or ```rust,quickstart).
    let open_a = "```rust quickstart";
    let open_b = "```rust,quickstart";
    let (idx, open) = readme
        .find(open_a)
        .map(|i| (i, open_a))
        .or_else(|| readme.find(open_b).map(|i| (i, open_b)))
        .expect(
            "README.md must contain a ```rust quickstart code block (the drift-guarded \
             quickstart). Tag the quickstart fence exactly ```rust quickstart.",
        );
    let after_fence = &readme[idx + open.len()..];
    // Skip to the end of the fence line, then take up to the closing ```.
    let after_nl = after_fence
        .split_once('\n')
        .expect("fence line must end with a newline")
        .1;
    let body = after_nl
        .split_once("\n```")
        .expect("README quickstart block must be closed with ```")
        .0;
    body.trim_matches('\n').to_string()
}

#[test]
fn readme_quickstart_matches_crate_source() {
    let from_crate = crate_quickstart_block();
    let from_readme = readme_quickstart_block();
    assert_eq!(
        from_readme, from_crate,
        "\n\nREADME quickstart block has DRIFTED from examples/quickstart/src/lib.rs.\n\
         The README is a tested artifact: update the ```rust quickstart block to match \
         the crate (or vice-versa).\n\n--- README ---\n{from_readme}\n\n--- CRATE ---\n{from_crate}\n"
    );
}
