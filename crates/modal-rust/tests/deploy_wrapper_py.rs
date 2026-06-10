//! H3 (architecture review 2026-06-10): EXECUTE the deploy wrapper Python.
//!
//! `src/deploy/wrapper.py` is the only Python that runs in a deployed container;
//! `py_compile` proves it parses, this harness proves it BEHAVES — envelope
//! passthrough, serve-child reuse + one-shot fallback, the web handler's
//! 200/422/500 mapping with REDACTED error bodies, and the snapshot prime's
//! strict/best-effort split. The behaviors live in `src/deploy/wrapper_test.py`
//! (stdlib-only: the runner and fastapi are faked), so this test only needs a
//! `python3` on PATH — the same interpreter the gate's `py_compile` step already
//! assumes.

use std::process::Command;

#[test]
fn deploy_wrapper_python_behaviors() {
    let test_py = concat!(env!("CARGO_MANIFEST_DIR"), "/src/deploy/wrapper_test.py");
    let out = Command::new("python3")
        .arg(test_py)
        .output()
        .expect("python3 must be on PATH (the gate's py_compile step already requires it)");
    assert!(
        out.status.success(),
        "wrapper_test.py failed ({}):\n--- stdout ---\n{}\n--- stderr ---\n{}",
        out.status,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}
