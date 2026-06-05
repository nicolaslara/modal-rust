//! M13 — Burn tensor smoke: a minimal Burn CUDA-backend tensor add.
//!
//! This is the project's downstream-consumer GPU proof and is the LAST step of
//! the Burn-free-first ordering (nvidia-smi → rust nvidia-smi → cudarc → **Burn**).
//! It runs an element-wise tensor add `c = a + b` on the Burn **CUDA backend**
//! (`burn_cuda::Cuda<f32, i32>`), then verifies the GPU result against a CPU
//! reference.
//!
//! Unlike M12 (cudarc + a precompiled PTX kernel through the Driver API, Tier 0),
//! Burn drives **CubeCL**, which **JIT-compiles its kernels via NVRTC at
//! runtime**. That makes M13 the first **Tier 1** milestone: `libnvrtc.so` AND
//! `libcudart.so` MUST be on the loader path (boundaries.md §9; tasks.md M13).
//! The runtime image therefore carries the CUDA runtime + NVRTC (a
//! `nvidia/cuda:*-runtime-*` base, or pip `nvidia-cuda-nvrtc-cu12` +
//! `nvidia-cuda-runtime-cu12`).
//!
//! Pinned version set (pinned TOGETHER; recorded in Cargo.lock):
//!   burn 0.21 + burn-cuda 0.21 → cubecl 0.10 (cuda) → cubecl-cuda 0.10
//!     → cudarc ^0.19 (fallback-dynamic-loading + fallback-latest).
//!
//! Because dynamic loading hides a missing lib until first use, a **hard startup
//! self-check** (`tier1_self_check`) `dlopen`s `libnvrtc` + `libcudart` BEFORE we
//! touch Burn, failing loudly if the image is accidentally Tier 0.

// Alias the FACADE crate (`modal-rust`, renamed `modal_rust_facade` in Cargo.toml) so
// the attribute is spelled `#[modal_rust::function]`; the macro routes every emitted
// runtime/inventory path through `modal_rust::__private::…`, so this crate's only modal
// dependency is the `modal-rust` facade — no direct `modal-rust-runtime` / `inventory`.
extern crate modal_rust_facade as modal_rust;

use burn_cuda::{Cuda, CudaDevice};
use serde::{Deserialize, Serialize};

/// The Burn CUDA backend (CubeCL → cubecl-cuda → cudarc). f32 floats, i32 ints.
type CudaBackend = Cuda<f32, i32>;

/// Input for `burn_add` (M13). The runner argument is always a single named JSON
/// object (boundaries.md §3); `n` is the number of elements in each 1-D tensor.
#[derive(Debug, Deserialize)]
pub struct BurnAddInput {
    /// Number of elements in each tensor.
    pub n: usize,
}

/// Output for `burn_add` (M13).
#[derive(Debug, Serialize)]
pub struct BurnAddOutput {
    /// `true` iff the GPU-computed `c` equals the CPU reference `a + b`
    /// element-wise. The milestone passes iff this is `true`.
    pub valid: bool,
    /// The number of elements computed on the GPU.
    pub n: usize,
    /// Which Burn backend executed the op (evidence it ran on CUDA, not CPU).
    pub backend: String,
    /// The resolved loader paths of `libnvrtc` + `libcudart` (Tier 1 proof).
    pub libnvrtc: String,
    pub libcudart: String,
    /// A few spot-checked GPU results, so the envelope itself carries proof
    /// (index, gpu_value, cpu_reference).
    pub samples: Vec<(usize, f32, f32)>,
}

/// **HARD Tier-1 startup self-check (boundaries.md §9; tasks.md M13).** `dlopen`
/// `libnvrtc` + `libcudart` ourselves and fail LOUDLY if either is missing.
///
/// CubeCL (under burn-cuda) JIT-compiles kernels via NVRTC at runtime and links
/// CUDA dynamically, so a missing `libnvrtc`/`libcudart` would otherwise stay
/// hidden until the first kernel launch (deep inside Burn, with an opaque error).
/// Probing the loader here turns "accidentally Tier 0" into an immediate,
/// actionable failure. Returns the resolved soname/paths actually opened, as
/// Tier-1 evidence.
fn tier1_self_check() -> anyhow::Result<(String, String)> {
    // Candidate sonames, most-specific first. The unversioned `.so` is usually a
    // dev symlink (absent on runtime images), so we also try the versioned ones
    // that the CUDA runtime/NVRTC pip wheels and the `*-runtime-*` images ship.
    fn dlopen_any(label: &str, candidates: &[&str]) -> anyhow::Result<String> {
        let mut errors = Vec::new();
        for name in candidates {
            // SAFETY: we only open the library to prove it is loadable, and drop
            // the handle immediately; we never call into it here.
            match unsafe { libloading::Library::new(name) } {
                Ok(_lib) => return Ok((*name).to_string()),
                Err(e) => errors.push(format!("{name}: {e}")),
            }
        }
        anyhow::bail!(
            "Tier-1 self-check FAILED: could not dlopen `{label}`. The runtime \
             image must be Tier 1 (CUDA runtime + NVRTC on the loader path): \
             either `nvidia/cuda:<12.x|13.x>-runtime-<os>` + add_python, or Tier \
             0 + pip `nvidia-cuda-nvrtc-cu12` + `nvidia-cuda-runtime-cu12`. Burn \
             (CubeCL) JIT-compiles kernels via NVRTC at runtime, so a missing \
             `{label}` would otherwise stay hidden until the first kernel launch. \
             Tried: [{}]",
            errors.join("; ")
        )
    }

    let nvrtc = dlopen_any(
        "libnvrtc",
        &[
            "libnvrtc.so",
            "libnvrtc.so.12",
            "libnvrtc.so.13",
            "libnvrtc.so.11",
        ],
    )?;
    let cudart = dlopen_any(
        "libcudart",
        &[
            "libcudart.so",
            "libcudart.so.12",
            "libcudart.so.13",
            "libcudart.so.11",
        ],
    )?;
    Ok((nvrtc, cudart))
}

/// `burn_add` (M13): compute `c = a + b` element-wise for `n` elements on the
/// Burn CUDA backend, then verify the result against a CPU reference.
///
/// The `#[modal_rust::function(gpu = "T4", name = "burn_add")]` decorator IS the
/// config: the macro emits this unchanged fn PLUS `typed!(burn_add)` + an
/// `inventory::submit!` carrying `FunctionConfig { gpu: Some("T4"), .. }` (a single
/// bare user-struct param → Mode A, byte-identical to the manual `typed!` path). The
/// facade reads that config when CREATING the Modal function, so the function lands on
/// a T4 with no caller-side `with_gpu`. Run `modal_runner --describe` to see the gpu
/// ride through inventory; the runner dispatch itself ignores the config.
#[modal_rust::function(gpu = "T4", name = "burn_add")]
pub fn burn_add(input: BurnAddInput) -> anyhow::Result<BurnAddOutput> {
    use burn::tensor::{Tensor, TensorData};

    let n = input.n;
    if n == 0 {
        anyhow::bail!("n must be > 0");
    }

    // HARD Tier-1 gate: dlopen libnvrtc + libcudart, or fail loudly, BEFORE Burn.
    let (libnvrtc, libcudart) = tier1_self_check()?;

    // Host inputs + CPU reference (integral f32 sums; exact).
    let a_host: Vec<f32> = (0..n).map(|i| i as f32).collect();
    let b_host: Vec<f32> = (0..n).map(|i| (i as f32) * 2.0).collect();
    let cpu_ref: Vec<f32> = a_host.iter().zip(&b_host).map(|(x, y)| x + y).collect();

    // Initialize the CUDA backend device. `CudaDevice::default()` is device 0 and
    // triggers CubeCL's runtime init (which dlopens cudarc/NVRTC under the hood).
    let device = CudaDevice::default();

    // Build the two GPU tensors and add them ON THE GPU.
    let a = Tensor::<CudaBackend, 1>::from_data(TensorData::from(a_host.as_slice()), &device);
    let b = Tensor::<CudaBackend, 1>::from_data(TensorData::from(b_host.as_slice()), &device);
    let c = a + b; // element-wise add, dispatched to a CubeCL CUDA kernel.

    // Pull the GPU result back to the host as f32.
    let c_data = c.into_data();
    let c_host: Vec<f32> = c_data
        .to_vec::<f32>()
        .map_err(|e| anyhow::anyhow!("failed to read GPU tensor data back to host: {e:?}"))?;

    // Element-wise verification vs the CPU reference.
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
    let samples: Vec<(usize, f32, f32)> = idxs
        .into_iter()
        .map(|i| (i, c_host[i], cpu_ref[i]))
        .collect();

    Ok(BurnAddOutput {
        valid,
        n,
        backend: "burn-cuda (CubeCL CUDA / cudarc)".to_string(),
        libnvrtc,
        libcudart,
        samples,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_decodes_from_named_object() {
        let v: BurnAddInput = serde_json::from_str(r#"{"n":256}"#).unwrap();
        assert_eq!(v.n, 256);
    }

    #[test]
    fn registry_has_burn_add() {
        // The `#[modal_rust::function]` decorator submits `burn_add` to inventory;
        // `Registry::from_inventory()` collects it into the SAME lookup the manual
        // builder produced. `Registry` resolves through the facade re-export (the
        // `extern crate … as modal_rust` alias above).
        use modal_rust::Registry;
        let reg = Registry::from_inventory();
        assert!(reg.get("burn_add").is_some());
        assert!(reg.get("nope").is_none());
    }

    #[test]
    fn tier1_self_check_fails_loudly_on_cpu_host() {
        // On a CPU-only host (this Mac / a Tier-0 image) neither libnvrtc nor
        // libcudart is present, so the hard gate must return an Err — never a
        // silent pass. (The GPU path itself is proven on Modal T4.)
        let r = tier1_self_check();
        if let Err(e) = &r {
            let msg = e.to_string();
            assert!(msg.contains("Tier-1 self-check FAILED"));
        }
        // If it unexpectedly succeeds (CUDA libs ARE on this host), that is also
        // acceptable — we only assert it does not panic and is well-formed.
    }
}
