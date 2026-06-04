//! The `add` walking-skeleton example (boundaries.md §3, tasks.md M0).
//!
//! The user authors only this `lib.rs` and `modal_registry()` — they do not own
//! `main()`. The CLI owns the ~15-line `src/bin/modal_runner.rs` whose fixed body
//! is `modal_rust_runtime::run_cli(example_add::modal_registry())`.
//!
//! Besides the real `add` handler, this registers named test entrypoints that
//! exercise every one of the five runner error kinds so the M0 acceptance commands
//! are reproducible — including one `Serialize`-error handler that populates the
//! envelope's `details`, and one anyhow handler whose `details` is `null`.

use anyhow::anyhow;
use modal_rust_runtime::{typed, Registry};
use serde::{Deserialize, Serialize};

/// The single named-JSON-object input for `add` (boundaries.md §3: never a
/// positional array).
///
/// Derives both `Serialize` and `Deserialize`: the runner only needs
/// `Deserialize` (it decodes the input), but the facade's `Function::local`/
/// `.remote()` callers construct and serialize an `In` value, so the input type
/// must also be `Serialize`. Adding `Serialize` is additive and changes no
/// behavior.
#[derive(Debug, Serialize, Deserialize)]
pub struct AddInput {
    /// First addend.
    pub a: i64,
    /// Second addend.
    pub b: i64,
}

/// The output of `add`.
///
/// Derives both `Serialize` and `Deserialize`: the runner only needs `Serialize`
/// (it encodes the output), but the facade's `Function::local`/`.remote()` callers
/// deserialize the handler's JSON output back into an `Out` value, so the output
/// type must also be `Deserialize`. Adding `Deserialize` is additive and changes
/// no behavior.
#[derive(Debug, Serialize, Deserialize)]
pub struct AddOutput {
    /// `a + b`.
    pub sum: i64,
}

/// Add two integers. The real entrypoint: returns `anyhow::Result` so its error
/// path (when used) lands on `function_error` with `details = null`.
pub fn add(input: AddInput) -> anyhow::Result<AddOutput> {
    Ok(AddOutput {
        sum: input.a + input.b,
    })
}

/// A structured, `Serialize` user error. When a handler returns this, the
/// monomorphized `typed!` wrapper preserves it structurally into the envelope's
/// `details` field (boundaries.md §2).
#[derive(Debug, Serialize)]
pub struct AddError {
    /// A machine-readable error code.
    pub code: u32,
    /// A human-readable reason.
    pub reason: String,
}

impl std::fmt::Display for AddError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "add error [{}]: {}", self.code, self.reason)
    }
}

impl std::error::Error for AddError {}

/// `function_error` with `details = null`: returns an opaque `anyhow` error.
pub fn fail(_input: AddInput) -> anyhow::Result<AddOutput> {
    Err(anyhow!("deliberate anyhow failure from `fail`"))
}

/// `function_error` with a populated `details`: returns a `Serialize` user error,
/// so `details = serde_json::to_value(&e)` (boundaries.md §2).
pub fn fail_structured(_input: AddInput) -> Result<AddOutput, AddError> {
    Err(AddError {
        code: 42,
        reason: "structured failure with details".to_string(),
    })
}

/// `encode_error` (NOT `panic`): produces an `Out` that fails to serialize. A map
/// keyed by a **tuple** has no JSON object representation (JSON object keys must be
/// strings, and serde_json refuses a non-string-convertible key with "key must be a
/// string"), so the codec's `encode` step returns `Err` -> `encode_error`.
///
/// (Note: `f64::NAN` would NOT work — serde_json silently encodes it as `null`; nor
/// would integer/bool keys, which serde_json coerces to string keys.)
#[derive(Debug, Serialize)]
pub struct BadOutput {
    /// A tuple-keyed map; serde_json cannot encode these keys as JSON object keys.
    pub by_pair: std::collections::BTreeMap<(i32, i32), i32>,
}

/// Returns a value whose `Out` cannot be serialized to JSON -> `encode_error`.
pub fn bad_encode(_input: AddInput) -> anyhow::Result<BadOutput> {
    let mut by_pair = std::collections::BTreeMap::new();
    by_pair.insert((1, 2), 3);
    Ok(BadOutput { by_pair })
}

/// `panic`: unwinds in the handler body. Captured via the panic hook +
/// `catch_unwind` into the `panic` envelope (message + backtrace). This is the
/// SAME entrypoint M4 reuses, so M0 and M4 stay consistent.
pub fn will_panic(_input: AddInput) -> anyhow::Result<AddOutput> {
    panic!("deliberate panic from `will_panic`");
}

/// The (empty) input for `gpu_info` (M11). The runner argument is always a single
/// named JSON object (boundaries.md §3); a fieldless struct deserializes cleanly
/// from `{}`, so M11 is driven with `--input '{}'`.
#[derive(Debug, Deserialize)]
pub struct GpuInfoInput {}

/// The output of `gpu_info`: the captured `nvidia-smi` report (M11).
#[derive(Debug, Serialize)]
pub struct GpuInfoOutput {
    /// `nvidia-smi`'s stdout — the GPU + driver version + CUDA Driver API version,
    /// now produced BY RUST.
    pub nvidia_smi: String,
    /// `nvidia-smi`'s exit code (0 on success).
    pub exit_code: i32,
    /// Any `nvidia-smi` stderr (normally empty).
    pub stderr: String,
}

/// `gpu_info` (M11): observe the GPU from RUST. Shells out to `nvidia-smi` via
/// `std::process::Command` and returns its stdout through the M0 JSON envelope.
///
/// This is the Burn-free, CUDA-crate-free GPU observation step (Tier 0): the GPU
/// machine preinstalls the NVIDIA driver + Driver API (`libcuda`) + `nvidia-smi`,
/// so no CUDA toolkit and no `cudarc` are needed. The ONLY new variable over the
/// CPU-proven M4/M7 build path is `gpu=` placement on the Function — the build
/// recipe is identical (boundaries.md §9, research-synthesis.md §3 M11).
pub fn gpu_info(_input: GpuInfoInput) -> anyhow::Result<GpuInfoOutput> {
    let output = std::process::Command::new("nvidia-smi").output()?;
    Ok(GpuInfoOutput {
        nvidia_smi: String::from_utf8_lossy(&output.stdout).into_owned(),
        exit_code: output.status.code().unwrap_or(-1),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}

/// The manual v0 registry (boundaries.md §3). Registers `add` plus the named test
/// entrypoints used to exercise the remaining error kinds. A future
/// `#[modal_rust::function]` macro path would generate the SAME `__wrap` fn and
/// register its pointer — the protocol is unchanged.
pub fn modal_registry() -> Registry {
    Registry::new()
        .function("add", typed!(add))
        .function("fail", typed!(fail))
        .function("fail_structured", typed!(fail_structured))
        .function("bad_encode", typed!(bad_encode))
        .function("will_panic", typed!(will_panic))
        .function("gpu_info", typed!(gpu_info))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_works() {
        let out = add(AddInput { a: 40, b: 2 }).unwrap();
        assert_eq!(out.sum, 42);
    }

    #[test]
    fn registry_has_all_entrypoints() {
        let reg = modal_registry();
        for name in [
            "add",
            "fail",
            "fail_structured",
            "bad_encode",
            "will_panic",
            "gpu_info",
        ] {
            assert!(reg.get(name).is_some(), "missing entrypoint {name}");
        }
        assert!(reg.get("nope").is_none());
    }

    #[test]
    fn gpu_info_input_decodes_from_empty_object() {
        // M11 is driven with `--input '{}'`; the fieldless input must decode from
        // an empty JSON object.
        let _: GpuInfoInput = serde_json::from_str("{}").unwrap();
    }
}
