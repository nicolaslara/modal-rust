# modal-rust-cli

The `modal-rust` command-line tool — `run`, `deploy`, and `call` Rust functions on
[Modal](https://modal.com), no Python and no codegen.

This crate provides the **binary**. The library you depend on to author functions is
[`modal-rust`](https://crates.io/crates/modal-rust).

## Install

Pre-release, so pin the version (`cargo install` skips pre-releases by default):

```bash
cargo install modal-rust-cli --version 0.1.0-alpha.2
```

Then, from a crate that authors `#[modal_rust::function]`s:

```bash
modal-rust deploy <entrypoint> --project .
modal-rust run    <entrypoint> --project . --input '{"a":2,"b":3}'
```

Full documentation: <https://github.com/nicolaslara/modal-rust>

License: MIT OR Apache-2.0
