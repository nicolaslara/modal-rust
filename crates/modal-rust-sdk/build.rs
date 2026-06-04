use std::error::Error;

fn main() -> Result<(), Box<dyn Error>> {
    let protoc_path = protoc_bin_vendored::protoc_bin_path()?;
    // SAFETY: build scripts run single-threaded before protobuf compilation.
    unsafe { std::env::set_var("PROTOC", protoc_path) };
    let protoc_include = protoc_bin_vendored::include_path()?;

    tonic_prost_build::configure()
        .build_client(true)
        .build_server(false) // client only — drops server codegen (and axum)
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
