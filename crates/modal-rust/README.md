# modal-rust

Deploy and run Rust functions on [Modal](https://modal.com) — no Python.

`modal-rust` is the user-facing **library**: author `#[modal_rust::function]`s, run them
in-process with `.local()`, or call the locked `.remote()` / `.spawn()` / `.map()` async
surface. The gRPC control-plane client is re-exported as `modal_rust::sdk` behind the
non-default `client` feature, so a function-authoring crate stays light (~9 crates).

## Install

This is a **pre-release**, so pin the version explicitly — `cargo add` skips
pre-releases by default:

```toml
[dependencies]
modal-rust = "0.1.0-alpha.2"
```

## The CLI

The `modal-rust` **binary** (`run` / `deploy` / `call`) ships in a separate crate,
[`modal-rust-cli`](https://crates.io/crates/modal-rust-cli) — so `cargo install modal-rust`
does **not** work (this crate is the library; same split as `wasmtime` / `wasmtime-cli`):

```bash
cargo install modal-rust-cli --version 0.1.0-alpha.2
```

Full documentation: <https://github.com/nicolaslara/modal-rust>

License: MIT OR Apache-2.0
