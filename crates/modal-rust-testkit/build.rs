//! Build the SERVER side of the `ModalClient` gRPC service from the (copied)
//! Modal proto. The testkit owns its OWN `build_server(true)` codegen (Option A)
//! so the in-process mock can implement the full 201-method service trait — the
//! handful the SDK calls are hand-written; the rest are macro-stubbed. The proto
//! is a verbatim copy of `crates/modal-rust-sdk/proto/api.proto` (kept in sync).

use std::error::Error;

fn main() -> Result<(), Box<dyn Error>> {
    println!("cargo:rerun-if-changed=proto/api.proto");

    let protoc_path = protoc_bin_vendored::protoc_bin_path()?;
    // SAFETY: build scripts run single-threaded before protobuf compilation.
    unsafe { std::env::set_var("PROTOC", protoc_path) };
    let protoc_include = protoc_bin_vendored::include_path()?;

    tonic_prost_build::configure()
        .build_client(false)
        .build_server(true) // server trait for the mock
        .compile_protos(
            &["proto/api.proto"],
            &[
                "proto",
                protoc_include
                    .to_str()
                    .ok_or("invalid protoc include path")?,
            ],
        )?;

    Ok(())
}
