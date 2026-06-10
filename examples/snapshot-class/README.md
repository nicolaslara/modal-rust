# snapshot-class

Pay an expensive `#[enter]` build **once, ever** with
`#[modal_rust::cls(enable_memory_snapshot = true)]`.

A plain `#[cls]` (see `examples/stateful-class`) runs `#[enter]` once per *warm*
container and reuses the result by `&self` — but a **cold** container, after
scale-to-zero, re-runs `#[enter]` from scratch. For a genuinely expensive build
that cold-start cost recurs forever. `enable_memory_snapshot = true` adds one
flag: a **deployed** app runs `#[enter]` once, Modal snapshots the loaded
process, and every later container — cold ones included — **restores** the
already-built state instead of re-running the build.

Here the expensive state is a sorted word-concordance index over an embedded
corpus (`src/concordance.rs`): `#[enter]` tokenizes the corpus, counts every
word, and sorts the result so `search` can binary-search it. The heavy build
lives in its own module so `src/lib.rs` stays the clean Modal surface:

```rust cls
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
```

Each method becomes its own dotted entrypoint (`Concordance.search`,
`Concordance.vocabulary`) carrying the merged `#[cls]` + `#[method]` config.

## The snapshot is deploy-only

Modal only snapshots **deployed** apps, so `enable_memory_snapshot` takes effect
on `deploy`, not `run`:

- On **deploy**, the flag rides into the wire field Modal reads to snapshot the
  class (`FunctionCreate.checkpointing_enabled`) — proven offline by
  `tests/manifest.rs`.
- On **run**, the flag is suppressed: the wire is byte-identical to a
  non-snapshot `#[cls]`, and `#[enter]` falls back to the ordinary load-once
  (once per warm container) behavior.

So you `run` to iterate locally and `deploy` to get the cold-start snapshot win.

**A failed prime fails loud.** If `#[enter]` errors (or panics) during the snapshot
prime, the container init **fails visibly at deploy time** instead of silently
re-running `#[enter]` on every cold start (a hidden perf cliff). To opt into
degrading — log the failure and fall back to lazy `#[enter]` on the first request —
set `MODAL_RUST_SNAPSHOT_BEST_EFFORT=1` at deploy time (or
`DeployConfig::snapshot_best_effort`).

## Deploy and call it

```bash
cd examples/snapshot-class
modal-rust deploy Concordance.search --app modal-rust-snapshot-class
modal-rust call Concordance.search --app modal-rust-snapshot-class --input '{"prefix":"wa"}'
```

```json
{"ok":true,"value":[{"word":"want","count":3},{"word":"was","count":1}]}
```

`Concordance.vocabulary` takes no input and reports the distinct-word count:

```bash
modal-rust call Concordance.vocabulary --app modal-rust-snapshot-class --input '{}'
```

```json
{"ok":true,"value":99}
```

You can also `run` it without deploying — it works exactly like a plain `#[cls]`
(load-once per warm container, no snapshot):

```bash
cd examples/snapshot-class
modal-rust run Concordance.search --input '{"prefix":"wa"}'
```

`Concordance.search` requires `--input` with a `prefix` field; the CLI validates
the input shape locally and fails fast (without calling Modal) if it does not
match.

## The frozen-`#[enter]`-state caveat

A memory snapshot freezes the **entire process state** at the moment `#[enter]`
finishes, and restores that exact state on every cold container. Anything
`#[enter]` captures is frozen identically across all restores:

- **Environment variables** read in `#[enter]` keep the builder's values.
- **The wall clock / timestamps** captured in `#[enter]` are frozen — a restored
  container reports the *snapshot* time, not its own start time.
- **RNG seeds** drawn in `#[enter]` replay identically on every restore.
- **Open connections / file handles** opened in `#[enter]` are captured in the
  snapshot; a restored container may need to re-establish them.

The snapshot is correct for the deterministic, self-contained index built here
(no env, no clock, no connections). The general rule: do **not** capture
per-container or time-sensitive values in `#[enter]` and expect them to differ
across restores. Per-container work that must be fresh belongs in the method
body (which runs on every call), not in `#[enter]`.

The GPU snap/restore split (load on CPU in the snapshot window, move to the GPU
*after* restore) and a `#[function]`-level snapshot are tracked as follow-ups in
`docs/ROADMAP.md`; v0 is CPU-only and `#[cls]`-only. See `docs/PARITY.md` for
current status.

## Prereqs

Modal credentials configured (`modal token new`). Run `modal-rust doctor --rust`
to check your environment first.
