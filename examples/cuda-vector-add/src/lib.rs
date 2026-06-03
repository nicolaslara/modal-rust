//! M12 — real Rust GPU COMPUTE: a cudarc vector-add via the CUDA Driver API.
//!
//! This is the project's first GPU *compute* proof and is deliberately
//! **Burn-free** (the Burn-free-first ordering: nvidia-smi → rust nvidia-smi →
//! **cudarc** → Burn). It runs `c[i] = a[i] + b[i]` for `n` elements on the GPU
//! using cudarc with the `dynamic-loading` feature (links with NO CUDA at build
//! time; dlopens `libcuda.so` at runtime), loading a **precompiled PTX** kernel
//! through the Driver API. The runtime image is **Tier 0** (driver-only): only
//! `libcuda` is needed — no `nvcc`, no runtime NVRTC, no `libcudart`
//! (boundaries.md §9; gpu-compute/tasks.md M12).
//!
//! The kernel is shipped as the checked-in `kernels/vector_add.ptx`
//! (`.target sm_52`), embedded at compile time via `include_str!`. PTX is the
//! driver-JIT, forward-compatible IR: the driver JIT-compiles it forward to the
//! actual GPU arch (T4 = sm_75) at module-load time — so it is NOT NVRTC-compiled
//! at runtime and NOT a fixed-arch cubin.

use modal_rust_runtime::{typed, Registry};
use serde::{Deserialize, Serialize};

/// The precompiled, checked-in PTX vector-add kernel, embedded at build time.
/// Loaded through the Driver API at runtime (needs only `libcuda`).
const VECTOR_ADD_PTX: &str = include_str!("../kernels/vector_add.ptx");

/// The kernel function name (`extern "C"` symbol) inside the PTX module.
const KERNEL_NAME: &str = "vector_add";

/// Input for `vector_add` (M12). The runner argument is always a single named
/// JSON object (boundaries.md §3); `n` is the number of elements.
#[derive(Debug, Deserialize)]
pub struct VectorAddInput {
    /// Number of elements in each vector.
    pub n: usize,
}

/// Output for `vector_add` (M12).
#[derive(Debug, Serialize)]
pub struct VectorAddOutput {
    /// `true` iff the GPU-computed `c` equals the CPU reference `a + b`
    /// element-wise. The milestone passes iff this is `true`.
    pub valid: bool,
    /// The number of elements computed on the GPU.
    pub n: usize,
    /// The GPU model as reported by the driver (e.g. "Tesla T4"), for evidence.
    pub gpu_name: String,
    /// The CUDA driver (Driver API) version reported by `libcuda`, for evidence
    /// (point-in-time; drifts — never asserted against a hardcoded value).
    pub driver_version: i32,
    /// A few spot-checked GPU results, so the envelope itself carries proof
    /// (index, gpu_value, cpu_reference).
    pub samples: Vec<(usize, f32, f32)>,
}

/// **Startup self-check (boundaries.md §9):** dlopen `libcuda` and fail LOUDLY if
/// it is missing or no device is present. With `dynamic-loading`, a missing
/// `libcuda` would otherwise stay hidden until first use — this surfaces it
/// immediately with an actionable message. Returns the initialized
/// `CudaContext` on success.
fn cuda_self_check() -> anyhow::Result<std::sync::Arc<cudarc::driver::CudaContext>> {
    // `CudaContext::new(0)` triggers the lazy dlopen of `libcuda` and CUDA
    // driver init for device 0. Any failure here (missing `libcuda`, no GPU,
    // driver/device mismatch) is the loud, hard gate the tiering plan requires.
    let ctx = cudarc::driver::CudaContext::new(0).map_err(|e| {
        anyhow::anyhow!(
            "CUDA self-check FAILED: could not dlopen `libcuda` / init the CUDA \
             Driver API on device 0 ({e:?}). The runtime image must be Tier 0 \
             (driver-only): `libcuda.so` + a real NVIDIA GPU are required. This is \
             the dynamic-loading footgun — a missing driver lib stays hidden until \
             first use, so we fail loudly at startup."
        )
    })?;
    Ok(ctx)
}

/// Strip the leading human-facing comment header from the checked-in PTX,
/// returning the assembly from the first `.version` directive onward. ptxas
/// rejects any non-ASCII byte even inside `//` comments, so we never hand the
/// driver our comment block. If no `.version` is found (unexpected), the input
/// is returned unchanged.
fn sanitize_ptx(ptx: &str) -> String {
    match ptx.find(".version") {
        Some(i) => ptx[i..].to_string(),
        None => ptx.to_string(),
    }
}

/// Diagnostic: re-attempt the PTX load via the raw Driver API with a JIT error
/// log buffer so a `CUDA_ERROR_INVALID_PTX` carries the driver's actual reason
/// (the safe `load_data` path discards the log). Best-effort — never panics.
fn capture_ptx_jit_error_log(
    ctx: &std::sync::Arc<cudarc::driver::CudaContext>,
    ptx: &str,
) -> String {
    use cudarc::driver::sys;
    // Ensure the context is current on this thread (mirrors safe load_module).
    let _ = ctx.bind_to_thread();
    let c_ptx = match std::ffi::CString::new(ptx) {
        Ok(s) => s,
        Err(_) => return "<PTX contained an interior NUL byte>".to_string(),
    };
    let mut err_buf = vec![0u8; 8192];
    let mut options = [
        sys::CUjit_option::CU_JIT_ERROR_LOG_BUFFER,
        sys::CUjit_option::CU_JIT_ERROR_LOG_BUFFER_SIZE_BYTES,
    ];
    let mut values: [*mut std::ffi::c_void; 2] = [
        err_buf.as_mut_ptr() as *mut std::ffi::c_void,
        err_buf.len() as *mut std::ffi::c_void,
    ];
    let mut module = std::ptr::null_mut();
    // SAFETY: valid out-ptr, option/value arrays of equal length, null-terminated PTX.
    let res = unsafe {
        sys::cuModuleLoadDataEx(
            &mut module,
            c_ptx.as_ptr() as *const std::ffi::c_void,
            options.len() as u32,
            options.as_mut_ptr(),
            values.as_mut_ptr(),
        )
    };
    let nul = err_buf.iter().position(|&b| b == 0).unwrap_or(err_buf.len());
    let log = String::from_utf8_lossy(&err_buf[..nul]).trim().to_string();
    if log.is_empty() {
        format!("<empty> (cuModuleLoadDataEx returned {res:?})")
    } else {
        log
    }
}

/// `vector_add` (M12): compute `c[i] = a[i] + b[i]` for `n` elements on the GPU
/// via the CUDA Driver API + a precompiled PTX kernel, then verify the result
/// element-wise against a CPU reference.
pub fn vector_add(input: VectorAddInput) -> anyhow::Result<VectorAddOutput> {
    use cudarc::driver::{LaunchConfig, PushKernelArg};
    use cudarc::nvrtc::Ptx;

    let n = input.n;
    if n == 0 {
        anyhow::bail!("n must be > 0");
    }

    // Hard startup self-check: dlopen libcuda + init driver, or fail loudly.
    let ctx = cuda_self_check()?;
    let stream = ctx.default_stream();

    // Evidence: GPU model + driver (Driver API) version, straight from libcuda.
    let gpu_name = ctx.name().unwrap_or_else(|_| "<unknown>".to_string());
    // `cuDriverGetVersion` is a libcuda (driver) symbol — Tier 0, no toolkit.
    let driver_version = {
        let mut v: std::ffi::c_int = -1;
        // SAFETY: passing a valid out-pointer; the dlopened libcuda symbol fills it.
        match unsafe { cudarc::driver::sys::cuDriverGetVersion(&mut v) } {
            cudarc::driver::sys::CUresult::CUDA_SUCCESS => v as i32,
            _ => -1,
        }
    };

    // Load the PRECOMPILED PTX through the Driver API. `Ptx::from_src` takes the
    // PTX text as-is (NO NVRTC compilation) — the driver JITs it to the device
    // arch at `cuModuleLoad` time. This is what keeps M12 Tier 0. On JIT failure
    // we surface the driver's JIT ERROR LOG (otherwise discarded) for diagnosis.
    //
    // We hand the driver only the assembly starting at the `.version` directive:
    // ptxas rejects any non-ASCII byte even inside leading `//` comments, so we
    // strip the (human-facing) comment header before loading. Belt-and-suspenders
    // alongside the checked-in file being ASCII-only.
    let ptx_src = sanitize_ptx(VECTOR_ADD_PTX);
    let module = match ctx.load_module(Ptx::from_src(ptx_src.clone())) {
        Ok(m) => m,
        Err(e) => {
            let jit_log = capture_ptx_jit_error_log(&ctx, &ptx_src);
            return Err(anyhow::anyhow!(
                "failed to load precompiled PTX module: {e:?}; JIT error log: {jit_log}"
            ));
        }
    };
    let kernel = module
        .load_function(KERNEL_NAME)
        .map_err(|e| anyhow::anyhow!("failed to load kernel `{KERNEL_NAME}` from PTX: {e:?}"))?;

    // Host inputs + CPU reference.
    let a_host: Vec<f32> = (0..n).map(|i| i as f32).collect();
    let b_host: Vec<f32> = (0..n).map(|i| (i as f32) * 2.0).collect();
    let cpu_ref: Vec<f32> = a_host.iter().zip(&b_host).map(|(x, y)| x + y).collect();

    // Host -> device, allocate output.
    let a_dev = stream
        .clone_htod(&a_host)
        .map_err(|e| anyhow::anyhow!("htod copy of a failed: {e:?}"))?;
    let b_dev = stream
        .clone_htod(&b_host)
        .map_err(|e| anyhow::anyhow!("htod copy of b failed: {e:?}"))?;
    let mut c_dev = stream
        .alloc_zeros::<f32>(n)
        .map_err(|e| anyhow::anyhow!("device alloc of c failed: {e:?}"))?;

    // Launch: kernel signature is (float* out, const float* a, const float* b, int n).
    let n_i32 = n as i32;
    let cfg = LaunchConfig::for_num_elems(n as u32);
    let mut builder = stream.launch_builder(&kernel);
    builder.arg(&mut c_dev);
    builder.arg(&a_dev);
    builder.arg(&b_dev);
    builder.arg(&n_i32);
    unsafe { builder.launch(cfg) }
        .map_err(|e| anyhow::anyhow!("kernel launch failed: {e:?}"))?;

    // Device -> host.
    let c_host = stream
        .clone_dtoh(&c_dev)
        .map_err(|e| anyhow::anyhow!("dtoh copy of c failed: {e:?}"))?;

    // Element-wise verification vs the CPU reference (exact: integral f32 sums).
    let valid = c_host.len() == cpu_ref.len()
        && c_host
            .iter()
            .zip(&cpu_ref)
            .all(|(g, r)| (g - r).abs() < 1e-3);

    // Spot-check samples for the envelope (first, middle, last).
    let mut idxs = vec![0usize];
    if n > 2 {
        idxs.push(n / 2);
    }
    idxs.push(n - 1);
    idxs.dedup();
    let samples: Vec<(usize, f32, f32)> =
        idxs.into_iter().map(|i| (i, c_host[i], cpu_ref[i])).collect();

    Ok(VectorAddOutput {
        valid,
        n,
        gpu_name,
        driver_version,
        samples,
    })
}

/// The manual v0 registry (boundaries.md §3). Registers the single GPU
/// `vector_add` entrypoint.
pub fn modal_registry() -> Registry {
    Registry::new().function("vector_add", typed!(vector_add))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ptx_is_embedded_and_names_the_kernel() {
        // The precompiled PTX is checked in and embedded at build time, and it
        // declares the kernel symbol we load. (Cannot run the GPU path on a
        // CPU-only host; that is proven on Modal T4.)
        assert!(VECTOR_ADD_PTX.contains(".entry vector_add"));
        assert!(VECTOR_ADD_PTX.contains(".target sm_"));
        assert!(VECTOR_ADD_PTX.contains(".version "));
        // It must be ready-to-load PTX (driver-JIT), not CUDA C that NVRTC would
        // compile at runtime. Real PTX device instructions, no NVRTC step.
        assert!(VECTOR_ADD_PTX.contains("st.global.f32"));
        assert!(VECTOR_ADD_PTX.contains("add.f32"));
    }

    #[test]
    fn sanitize_strips_comment_header_to_version() {
        // The assembly handed to the driver must start at `.version` and contain
        // no leading comment bytes (ptxas rejects non-ASCII even in comments).
        let cleaned = sanitize_ptx(VECTOR_ADD_PTX);
        assert!(cleaned.starts_with(".version"));
        assert!(!cleaned.contains("PROVENANCE"));
        assert!(cleaned.is_ascii(), "PTX handed to the driver must be ASCII");
        assert!(cleaned.contains(".entry vector_add"));
    }

    #[test]
    fn input_decodes_from_named_object() {
        let v: VectorAddInput = serde_json::from_str(r#"{"n":1024}"#).unwrap();
        assert_eq!(v.n, 1024);
    }

    #[test]
    fn registry_has_vector_add() {
        let reg = modal_registry();
        assert!(reg.get("vector_add").is_some());
        assert!(reg.get("nope").is_none());
    }
}
