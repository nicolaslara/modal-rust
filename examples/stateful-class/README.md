# stateful-class

Load-once-serve-many with `#[modal_rust::cls]`. An `#[enter]` method loads an
expensive resource (here an embedding model) **once** per warm container; each
`#[method]` reuses it by `&self`. Each method becomes its own dotted entrypoint
(`Embedder.embed` with `gpu = "A10G"`, `Embedder.dim` with `gpu = "T4"`), with
merged class + method-override config proven offline by `tests/manifest.rs`.

## Run it

This example is directly runnable — no deploy required. The `#[enter]` load runs
on the **first method call in any container**, on the `run` path as well as the
`deploy` path, and is then reused by every later call in that warm container. The
`dim` method takes no input:

```bash
cd examples/stateful-class
modal-rust run Embedder.dim --input '{}'
```

```json
{"ok":true,"value":8}
```

`Embedder.embed` takes a `text` field and returns a fixed-width 8-element
unit-length embedding vector:

```bash
cd examples/stateful-class
modal-rust run Embedder.embed --input '{"text":"hello"}'
```

```json
{"ok":true,"value":[...]}
```

`Embedder.embed` requires `--input` with a `text` field. The CLI validates the
input shape locally and fails fast (without calling Modal) if it does not match.

The GPU on the decorator (`gpu = "T4"` class default, `gpu = "A10G"` on `embed`)
requests accelerator hardware for the container; it does **not** force a deploy.
GPU does not imply deploy in Modal, and it does not here — this example runs on
the `run` path as shown above.

## Why you might deploy this (and why you might not need to)

For *this* crate the heavy-build justification that applies to the GPU examples
does **not** apply. `src/embedding.rs` `Model::load()` is std-library
char-trigram hashing: no model download, no network, CPU-only, fully offline and
near-instant. So the in-body `cargo` build cost is small and there is no real
load latency to amortize — `run` is genuinely fine here.

When you *would* reach for `deploy` is the general per-example reason, not a GPU
rule:

- **In-body build cost on the `run` path.** `modal-rust run` builds the runner
  binary **in the function container at call time**, so a cold call pays a
  `cargo` compile. For a heavy crate that build can be large enough to be
  OOM-killed on a default container (you would see `GENERIC_STATUS_TERMINATED`
  with no error output) — which is why heavy examples set `memory =`. `deploy`
  builds the binary **once, at image-build time**, with full build resources, so
  deployed containers never recompile on a cold start. This crate's build is
  light, so neither concern bites here.
- **Warm `--serve` amortization for a `Cls`.** The load-once win only pays off
  while a container stays warm. The runner adds an additive `modal_runner
  --serve` loop that keeps the process alive across inputs, so `#[enter]` fires
  once and every subsequent call reuses the in-memory singleton. With a *real*
  expensive `#[enter]` (model weights, a connection pool), deploying and keeping
  a container warm is what turns "load once" into a sustained win across many
  calls; with this trivial load there is little to amortize.

If you do deploy, it is the same dotted entrypoint name:

```bash
cd examples/stateful-class
modal-rust deploy Embedder.embed --app modal-rust-stateful-class
modal-rust call Embedder.embed --app modal-rust-stateful-class --input '{"text":"hello"}'
```

## Making `#[enter]` cheaper when the load *is* expensive

When `#[enter]` does something genuinely costly, three Modal features compose
with this pattern to shrink the cost:

- **Volume (persistent data, no re-download).** Mount a `Volume` and have
  `#[enter]` read weights/caches from it instead of re-downloading on every cold
  container. The first container populates the volume; later containers — even
  cold ones — read the already-present data, so `#[enter]` skips the network.
- **Memory snapshots.** A memory snapshot would let `#[enter]` pay its expensive
  load **once, ever** and restore the warmed process state on every container
  start (including cold ones), instead of re-running the load per warm container.
  That is the highest-value future win for this `Cls` pattern (tracked in
  `docs/ROADMAP.md`); see `docs/PARITY.md` for current status.
- **`.map([..])` within a warm container.** Fan a batch of inputs through one
  method with `.map([..])`: the inputs share the same warm container, so
  `#[enter]` loads the model once and all mapped calls reuse that single
  in-memory singleton — amortizing the load-once cost across the whole batch.

## Prereqs

Modal credentials configured (`modal token new`); GPU access for the
`A10G`/`T4`-tagged methods. Run `modal-rust doctor --rust` to check your
environment first.
