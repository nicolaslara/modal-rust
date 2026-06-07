//! The `--describe` MANIFEST CACHE (describe-speed Part A.3).
//!
//! `modal-rust run`/`deploy` build the user crate's `modal_runner` LOCALLY only to run
//! `modal_runner --describe` and read the entrypoint manifest. That build + exec is the
//! sole thing the CLI needs from the local build — so caching the manifest bytes lets a
//! HIT skip BOTH the cargo build AND the `--describe` exec (0s).
//!
//! ## Key
//!
//! A SHA-256 over, in order:
//! 1. `DESCRIBE_CACHE_VERSION` — bumped when the manifest SHAPE changes, invalidating all
//!    entries.
//! 2. the resolved `package` name + the `is_generatable` decision (own-bin vs shadow), so
//!    flipping that path re-describes.
//! 3. the workspace `Cargo.lock` bytes (dep graph + versions).
//! 4. every `*.rs` + `Cargo.toml` in the cargo-metadata CLOSURE of `<package>` (the SAME
//!    closure the upload/shadow build use, via `modal_rust::describe_cache_inputs`), each
//!    hashed as `(workspace-relative-posix-path, file-bytes)` in sorted path order.
//!
//! The closure SOURCE is hashed (not `cargo metadata` output) because metadata is stable
//! across the source edits that change a `#[function]` config; source hashing catches
//! exactly those, and `Cargo.lock` (3) covers dependency/version changes. Nothing OUTSIDE
//! the closure can change the registry/configs the runner emits.
//!
//! ## Path + invalidation
//!
//! Entries live at `<shared_target>/.modal-rust/describe-<hash>.json` (gitignored via the
//! existing `target/` ignore, and travels with the build artifacts it mirrors). Any change
//! to a closure `.rs`/`Cargo.toml` or to `Cargo.lock` yields a different hash → miss →
//! rebuild → new file. Stale entries are not deleted inline (a `cargo clean` removes
//! them).
//!
//! ## Safety
//!
//! - If the key cannot be computed (metadata unavailable, an unreadable file), `key`
//!   returns `None` → no caching, exactly today's build-every-time behavior. Never errors.
//! - `load`/`store` are best-effort (IO errors swallowed) so a read-only target never
//!   breaks the command.
//! - The CALLER re-validates loaded bytes through the SAME `serde_json` parse +
//!   `check_schema` the live path uses, so a corrupt/incompatible cache file degrades to a
//!   rebuild, never a bad manifest.

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

/// Bump when the cached manifest SHAPE changes (invalidates every existing entry).
const DESCRIBE_CACHE_VERSION: u32 = 1;

/// A computed cache key: the hex SHA-256 over the version tag + package/path decision +
/// `Cargo.lock` + the closure source. Opaque; only [`load`]/[`store`] consume it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheKey(String);

impl CacheKey {
    /// The on-disk file name for this key: `describe-<hex>.json`.
    fn file_name(&self) -> String {
        format!("describe-{}.json", self.0)
    }
}

/// Compute the cache key for `package` rooted at `workspace_root` with the resolved
/// `is_generatable` decision, or `None` if any input cannot be read (→ caller skips
/// caching and builds, exactly the prior behavior). NEVER errors.
pub fn key(workspace_root: &Path, package: &str, is_generatable: bool) -> Option<CacheKey> {
    // The closure dirs whose source can change the `--describe` output (the SAME closure
    // the shadow/upload build use). `None` on any metadata failure → no caching.
    let dirs = modal_rust::describe_cache_inputs(workspace_root, package)?;

    let mut hasher = Sha256::new();
    // (1) version tag.
    hasher.update(DESCRIBE_CACHE_VERSION.to_le_bytes());
    // (2) package + path decision. Length-prefixed so distinct (package, flag) pairs can
    //     never collide via concatenation.
    update_field(&mut hasher, package.as_bytes());
    hasher.update([is_generatable as u8]);
    // (3) Cargo.lock bytes (absent → hash a sentinel so presence/absence is distinguished
    //     rather than silently ignored).
    let lock = workspace_root.join("Cargo.lock");
    match std::fs::read(&lock) {
        Ok(bytes) => {
            hasher.update([1u8]); // present
            update_field(&mut hasher, &bytes);
        }
        Err(_) => hasher.update([0u8]), // absent
    }
    // (4) closure source: every `*.rs` + `Cargo.toml`, by (rel-posix-path, bytes), sorted.
    let mut files = collect_closure_files(workspace_root, &dirs)?;
    files.sort_by(|a, b| a.0.cmp(&b.0));
    hasher.update((files.len() as u64).to_le_bytes());
    for (rel, abs) in &files {
        let bytes = std::fs::read(abs).ok()?; // unreadable closure file → no caching
        update_field(&mut hasher, rel.as_bytes());
        update_field(&mut hasher, &bytes);
    }

    Some(CacheKey(hex(&hasher.finalize())))
}

/// Load the cached manifest bytes for `key` under `shared_target`, or `None` on a miss /
/// any IO error (best-effort — a read failure degrades to a rebuild, never a hard error).
pub fn load(shared_target: &Path, key: &CacheKey) -> Option<Vec<u8>> {
    std::fs::read(cache_path(shared_target, key)).ok()
}

/// Store `manifest_bytes` for `key` under `shared_target`. Best-effort: any IO error
/// (e.g. a read-only target) is swallowed so the command never fails on a cache write.
pub fn store(shared_target: &Path, key: &CacheKey, manifest_bytes: &[u8]) {
    let path = cache_path(shared_target, key);
    if let Some(parent) = path.parent() {
        if std::fs::create_dir_all(parent).is_err() {
            return;
        }
    }
    let _ = std::fs::write(path, manifest_bytes);
}

/// `<shared_target>/.modal-rust/describe-<hash>.json`.
fn cache_path(shared_target: &Path, key: &CacheKey) -> PathBuf {
    shared_target.join(".modal-rust").join(key.file_name())
}

/// Hash one variable-length field with an 8-byte little-endian length prefix so adjacent
/// fields cannot run together (`"ab" + "c"` must differ from `"a" + "bc"`).
fn update_field(hasher: &mut Sha256, bytes: &[u8]) {
    hasher.update((bytes.len() as u64).to_le_bytes());
    hasher.update(bytes);
}

/// Walk each closure dir for `*.rs` + `Cargo.toml`, returning `(workspace-relative-posix
/// path, absolute path)` pairs. Prunes `target/` + `.git/` (the SAME floor the shadow
/// copy uses). A dir OUTSIDE `workspace_root` (an external path-dep) is keyed by its own
/// canonical-ish absolute path so it still contributes to the hash deterministically.
/// Returns `None` if a directory cannot be read (→ no caching).
fn collect_closure_files(
    workspace_root: &Path,
    dirs: &[PathBuf],
) -> Option<Vec<(String, PathBuf)>> {
    let mut out: Vec<(String, PathBuf)> = Vec::new();
    for dir in dirs {
        // The prefix each file's relative key is computed against: the workspace root when
        // the dir is inside it, else the dir's own parent (so an out-of-workspace path-dep
        // still produces a stable, dir-scoped key).
        let key_root = if dir.starts_with(workspace_root) {
            workspace_root.to_path_buf()
        } else {
            dir.clone()
        };
        walk_dir(dir, &key_root, &mut out)?;
    }
    Some(out)
}

/// Recursively collect `*.rs` + `Cargo.toml` under `dir`, keyed relative to `key_root`
/// (POSIX). Prunes `target/` + `.git/`. `None` on a read error.
fn walk_dir(dir: &Path, key_root: &Path, out: &mut Vec<(String, PathBuf)>) -> Option<()> {
    let entries = std::fs::read_dir(dir).ok()?;
    for entry in entries {
        let entry = entry.ok()?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let file_type = entry.file_type().ok()?;
        let path = entry.path();
        if file_type.is_dir() {
            if name == "target" || name == ".git" {
                continue; // build artifacts / VCS never affect the describe output
            }
            walk_dir(&path, key_root, out)?;
        } else if file_type.is_file() && is_cache_input(&name) {
            let rel = path
                .strip_prefix(key_root)
                .unwrap_or(&path)
                .components()
                .filter_map(|c| c.as_os_str().to_str())
                .collect::<Vec<_>>()
                .join("/");
            out.push((rel, path));
        }
        // Symlinks/other entries are skipped (source trees rarely need them; mirrors the
        // shadow copy floor).
    }
    Some(())
}

/// A file whose CONTENT can change the runner's `--describe` output: a Rust source file or
/// a `Cargo.toml` (a `#[function]` config edit lives in `.rs`; a dep/feature edit in
/// `Cargo.toml`; `Cargo.lock` is hashed separately at the workspace root).
fn is_cache_input(name: &str) -> bool {
    name == "Cargo.toml" || name.ends_with(".rs")
}

/// Lowercase-hex encode a digest (no extra dep).
fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// A self-contained single-crate workspace fixture (root `Cargo.toml` declares
    /// `[workspace]` + `[package]` so it is its own sole member) with a `src/lib.rs`. Used
    /// by the key tests; cleaned up by the caller.
    fn fixture(tag: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "mr-describe-cache-{tag}-{}-{}",
            std::process::id(),
            counter()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(
            root.join("Cargo.toml"),
            "[workspace]\nmembers = [\".\"]\n\n[package]\nname = \"fixturecrate\"\n\
             version = \"0.0.0\"\nedition = \"2021\"\n",
        )
        .unwrap();
        fs::write(root.join("src").join("lib.rs"), "pub fn f() -> i32 { 1 }\n").unwrap();
        fs::write(root.join("Cargo.lock"), "# lock v1\n").unwrap();
        root
    }

    fn counter() -> u64 {
        use std::sync::atomic::{AtomicU64, Ordering};
        static C: AtomicU64 = AtomicU64::new(0);
        C.fetch_add(1, Ordering::Relaxed)
    }

    #[test]
    fn cache_path_is_under_dot_modal_rust() {
        let key = CacheKey("deadbeef".to_string());
        let p = cache_path(Path::new("/ws/target"), &key);
        assert_eq!(
            p,
            PathBuf::from("/ws/target/.modal-rust/describe-deadbeef.json")
        );
    }

    #[test]
    fn store_then_load_round_trips() {
        let target = std::env::temp_dir().join(format!(
            "mr-describe-cache-rt-{}-{}",
            std::process::id(),
            counter()
        ));
        let key = CacheKey("cafef00d".to_string());
        assert!(load(&target, &key).is_none(), "miss before store");
        store(&target, &key, br#"{"schema":"modal-rust/describe@1"}"#);
        assert_eq!(
            load(&target, &key).as_deref(),
            Some(&br#"{"schema":"modal-rust/describe@1"}"#[..]),
            "stored bytes load back verbatim"
        );
        let _ = fs::remove_dir_all(&target);
    }

    #[test]
    fn load_misses_on_unknown_key_and_missing_target() {
        // A non-existent target dir is a clean miss (no panic, no error).
        let key = CacheKey("00".to_string());
        assert!(load(Path::new("/no/such/target/dir"), &key).is_none());
    }

    #[test]
    fn key_is_stable_for_unchanged_source() {
        let root = fixture("stable");
        let k1 = key(&root, "fixturecrate", true).expect("key computed");
        let k2 = key(&root, "fixturecrate", true).expect("key computed");
        assert_eq!(k1, k2, "same source + lock + decision => same key");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn key_changes_when_source_changes() {
        let root = fixture("src");
        let before = key(&root, "fixturecrate", true).expect("key");
        // Edit a closure `.rs` (e.g. a `#[function]` config change).
        fs::write(root.join("src").join("lib.rs"), "pub fn f() -> i32 { 2 }\n").unwrap();
        let after = key(&root, "fixturecrate", true).expect("key");
        assert_ne!(before, after, "a `.rs` edit must invalidate the key");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn key_changes_when_cargo_lock_changes() {
        let root = fixture("lock");
        let before = key(&root, "fixturecrate", true).expect("key");
        fs::write(root.join("Cargo.lock"), "# lock v2 (a dep bumped)\n").unwrap();
        let after = key(&root, "fixturecrate", true).expect("key");
        assert_ne!(before, after, "a Cargo.lock edit must invalidate the key");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn key_changes_when_cargo_toml_changes() {
        let root = fixture("toml");
        let before = key(&root, "fixturecrate", true).expect("key");
        // Add a dependency line (a closure `Cargo.toml` edit).
        fs::write(
            root.join("Cargo.toml"),
            "[workspace]\nmembers = [\".\"]\n\n[package]\nname = \"fixturecrate\"\n\
             version = \"0.0.0\"\nedition = \"2021\"\n[dependencies]\nserde = \"1\"\n",
        )
        .unwrap();
        let after = key(&root, "fixturecrate", true).expect("key");
        assert_ne!(before, after, "a Cargo.toml edit must invalidate the key");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn key_changes_when_generatable_flips() {
        let root = fixture("gen");
        let gen = key(&root, "fixturecrate", true).expect("key");
        let own = key(&root, "fixturecrate", false).expect("key");
        assert_ne!(
            gen, own,
            "flipping the own-bin/shadow decision re-describes (distinct key)"
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn key_is_none_when_metadata_unavailable() {
        // No Cargo.toml at the root → cargo metadata cannot resolve → no caching.
        let empty = std::env::temp_dir().join(format!(
            "mr-describe-cache-empty-{}-{}",
            std::process::id(),
            counter()
        ));
        fs::create_dir_all(&empty).unwrap();
        assert!(
            key(&empty, "whatever", true).is_none(),
            "missing manifest => None (build-every-time fallback)"
        );
        let _ = fs::remove_dir_all(&empty);
    }

    #[test]
    fn end_to_end_hit_after_store_with_real_key() {
        // A miss → store → hit cycle keyed by a REAL computed key over the fixture, with a
        // simulated shared target dir. Then a source edit recomputes a DIFFERENT key whose
        // entry is absent (a miss), proving invalidation reaches the on-disk lookup.
        let root = fixture("e2e");
        let target = root.join("target");

        let k = key(&root, "fixturecrate", true).expect("key");
        assert!(load(&target, &k).is_none(), "cold: miss");
        store(
            &target,
            &k,
            br#"{"schema":"modal-rust/describe@1","entrypoints":[]}"#,
        );
        assert_eq!(
            load(&target, &k).as_deref(),
            Some(&br#"{"schema":"modal-rust/describe@1","entrypoints":[]}"#[..]),
            "warm: hit returns the stored manifest"
        );

        // Edit source → new key → that entry is not present → miss (rebuild path).
        fs::write(
            root.join("src").join("lib.rs"),
            "pub fn f() -> i32 { 99 }\n",
        )
        .unwrap();
        let k2 = key(&root, "fixturecrate", true).expect("key");
        assert_ne!(k, k2, "edited source => different key");
        assert!(
            load(&target, &k2).is_none(),
            "the new key has no stored entry => miss => rebuild"
        );
        let _ = fs::remove_dir_all(&root);
    }
}
