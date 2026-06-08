//! `examples/snapshot-class` — pay the expensive `#[enter]` build ONCE, EVER.
//!
//! Teaching ONE concept: a memory-snapshot Cls, `#[modal_rust::cls(enable_memory_snapshot
//! = true)]`. It is a plain load-once `#[cls]` (see `examples/stateful-class`) plus one
//! flag that changes WHEN the expensive `#[enter]` load is paid.
//!
//! A plain `#[cls]` runs `#[enter]` once per warm container and reuses the result by
//! `&self` — but a COLD container (after scale-to-zero) re-runs `#[enter]` from scratch.
//! For a genuinely expensive build that cold-start cost recurs forever. With
//! `enable_memory_snapshot = true`, a DEPLOYED app runs `#[enter]` once, Modal snapshots
//! the loaded process, and every later container — including cold ones — RESTORES the
//! already-built state instead of re-running the build. The load-once win extends across
//! cold starts, not just within one warm container.
//!
//! Here the expensive state is a sorted word-concordance [`Index`] over an embedded text
//! corpus (built in [`concordance`]): `#[enter]` tokenizes the corpus, counts every
//! word, and sorts the result so queries binary-search it. A real app would build a far
//! larger index (or load model weights); the shape is the same.
//!
//! - `#[enter] fn load() -> Result<Self>` runs ONCE and builds the [`Index`]; the macro
//!   moves it into a process-lifetime singleton. On a snapshot deploy this build is what
//!   the snapshot freezes.
//! - `#[method] fn search(&self, prefix)` queries the loaded index by `&self` — a binary
//!   search over the precomputed sorted vector. `#[method] fn vocabulary(&self)` reports
//!   the distinct-word count. Both reuse the same in-memory singleton.
//!
//! DEPLOY-ONLY effect: Modal only snapshots DEPLOYED apps, so the flag takes effect on
//! `deploy`, not `run`. It rides into the DEPLOY `FunctionCreate.checkpointing_enabled`
//! (proven offline by `tests/manifest.rs`); on `run` the wire is byte-identical to a
//! non-snapshot Cls.
//!
//! FROZEN-`#[enter]`-STATE CAVEAT: anything `#[enter]` captures — env vars, the wall
//! clock, RNG seeds, open connections — is frozen IN the snapshot and restored
//! identically on every cold container. The snapshot is correct for the deterministic
//! index built here, but do NOT capture per-container or time-sensitive values in
//! `#[enter]` and expect them to differ across restores.
//!
//! The heavy build lives in its own module ([`concordance`]) so this file stays the clean
//! Modal surface — a plain `Concordance` struct plus the `#[cls]` impl that loads and
//! queries it. `tests/local.rs` proves OFFLINE that `#[enter]` runs exactly once across
//! many `.local()` calls and that the search is real; `tests/manifest.rs` proves
//! `enable_memory_snapshot` rides into the DEPLOY `FunctionCreate.checkpointing_enabled`
//! and NOT into the RUN manifest.

mod concordance;

use modal_rust::cls;

pub use concordance::Entry;
use concordance::Index;
// Re-exported for `tests/local.rs`: lets the offline test PROVE `#[enter]` (which calls
// `Index::build`) ran exactly once across many method calls — the load-once win.
pub use concordance::build_count;

/// The expensive state, built ONCE per warm container (and, on a snapshot deploy, frozen
/// into the memory snapshot so even cold containers restore it). A PLAIN struct — no
/// macro on the type. It just holds the loaded concordance [`Index`]; all class-level
/// config rides on the `#[cls(..)]` attribute on the impl block below.
//
// README drift-guard: the region between the begin/end markers below is kept
// byte-identical to the ```rust cls block in README.md (see tests/readme_drift.rs).
// cls:begin
pub struct Concordance {
    index: Index,
}

#[cls(enable_memory_snapshot = true, timeout = 600)] // memory-snapshot Cls (deploy-only effect).
impl Concordance {
    /// Runs ONCE to build the expensive state (mirrors `@modal.enter()`). On a DEPLOYED
    /// snapshot app this build runs a single time EVER: Modal snapshots the loaded process
    /// and restores it on every later container, cold ones included, so `#[enter]` is not
    /// re-run per cold start. (`Index::build` is offline + CPU-only here, but it stands in
    /// for the real "read a large index / model weights" cost.)
    #[enter]
    fn load() -> anyhow::Result<Self> {
        Ok(Concordance {
            index: Index::build(),
        })
    }

    /// Return every concordance entry whose word starts with `prefix`, reusing the
    /// already-built index by `&self` (a binary search over the precomputed sorted
    /// vector). Inherits `timeout = 600` from `#[cls]`.
    #[method]
    fn search(&self, prefix: String) -> anyhow::Result<Vec<Entry>> {
        Ok(self.index.search(&prefix))
    }

    /// Report how many distinct words the loaded index holds, reusing it by `&self`.
    #[method]
    fn vocabulary(&self) -> anyhow::Result<usize> {
        Ok(self.index.distinct_words())
    }
}
// cls:end
