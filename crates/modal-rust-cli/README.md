# modal-rust-cli

The `modal-rust` command-line tool runs, deploys, and calls Rust functions on
[Modal](https://modal.com).

This crate provides the **binary**. The library you depend on to author functions is
[`modal-rust`](https://crates.io/crates/modal-rust).

The CLI generates a Rust runner when your crate does not provide one and
provisions the Python adapters used inside Modal. You do not need to write
Python application code or install Modal's Python CLI locally. See the SDK's
[known invocation limits](../modal-rust-sdk/README.md#known-invocation-limits).

## Install

Pre-release, so pin the version (`cargo install` skips pre-releases by default):

```bash
cargo install modal-rust-cli --version 0.1.0-alpha.4
```

Then, from a crate that authors `#[modal_rust::function]`s:

```bash
modal-rust deploy <entrypoint> --project .
modal-rust run    <entrypoint> --project . --input '{"a":2,"b":3}'
```

Full documentation: <https://github.com/nicolaslara/modal-rust>

License: MIT OR Apache-2.0
