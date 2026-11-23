# modal-rust

Deploy and run Rust functions on [Modal](https://modal.com) with a Rust authoring API.

`modal-rust` is the user-facing **library**: author `#[modal_rust::function]`s, run them
in-process with `.local()`, or use the `.remote()` / `.spawn()` / `.map()` async
surface. The gRPC control-plane client is re-exported as `modal_rust::sdk` behind the
non-default `client` feature, so a function-authoring crate can exclude the gRPC stack.

Remote execution uses a Python adapter inside Modal to communicate with the Rust
runner. You do not need to write Python application code or install Modal's
Python CLI locally. See the SDK's
[known invocation limits](../modal-rust-sdk/README.md#known-invocation-limits)
for concurrency, handler errors, payload sizes, and retry/cancellation behavior.

## Install

This is a **pre-release**, so pin the version explicitly — `cargo add` skips
pre-releases by default:

```toml
[dependencies]
modal-rust = "0.1.0-alpha.4"
```

## The CLI

The `modal-rust` **binary** (`run` / `deploy` / `call`) ships in a separate crate,
[`modal-rust-cli`](https://crates.io/crates/modal-rust-cli) — so `cargo install modal-rust`
does **not** work (this crate is the library; same split as `wasmtime` / `wasmtime-cli`):

```bash
cargo install modal-rust-cli --version 0.1.0-alpha.4
```

Full documentation: <https://github.com/nicolaslara/modal-rust>

License: MIT OR Apache-2.0
