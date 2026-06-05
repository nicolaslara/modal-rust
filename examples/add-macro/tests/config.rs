//! Decorator-config + proof-body coverage for `examples/add-macro`, kept in a tests/
//! file so the headline `lib.rs` stays clean. The `add_gpu`/`add_extras` configs are
//! also asserted end-to-end in `crates/modal-rust`'s `App` tests; here we pin the
//! inventory `Registration.config` directly and exercise the offline proof body.

use example_add_macro::proof::{secret_vol_probe, ProbeInput};
use modal_rust::__private::inventory;
use modal_rust::{FunctionConfig, Registration};

fn registration(name: &str) -> Option<&'static Registration> {
    inventory::iter::<Registration>
        .into_iter()
        .find(|r| r.name == name)
}

#[test]
fn bare_macro_config_is_default() {
    let reg = registration("add").expect("macro must register `add`");
    assert_eq!(reg.config, FunctionConfig::default());
    assert!(reg.config.secrets.is_empty());
    assert!(reg.config.volumes.is_empty());
}

#[test]
fn gpu_decorator_parses_into_config() {
    let reg = registration("add_gpu").expect("macro must register `add_gpu`");
    assert_eq!(reg.config.gpu, Some("T4"));
    assert_eq!(reg.config.timeout_secs, Some(1800));
    assert_eq!(reg.config.cache, Some(false));
}

#[test]
fn secrets_and_volumes_decorator_parses_into_config() {
    let reg = registration("add_extras").expect("macro must register `add_extras`");
    assert_eq!(reg.config.secrets, &["my-secret"]);
    assert_eq!(reg.config.volumes, &[("/data", "my-vol")]);
    // The user volume mount never collides with the cargo cache `/cache`.
    for (mount, _name) in reg.config.volumes {
        assert_ne!(*mount, "/cache");
    }
}

#[test]
fn macro_captures_user_package_for_auto_detect() {
    // PACKAGE AUTO-DETECT (P2): the `#[modal_rust::function]` macro captured THIS
    // crate's `env!("CARGO_PKG_NAME")` into every registration, so `.remote()` builds
    // `cargo build -p example-add-macro` automatically — no `MODAL_RUST_PACKAGE`.
    let reg = registration("add").expect("macro must register `add`");
    assert_eq!(reg.package, "example-add-macro");
    // Every decorated handler in one crate carries the SAME package.
    for name in ["add_gpu", "add_extras", "secret_vol_probe"] {
        let r = registration(name).unwrap_or_else(|| panic!("missing {name}"));
        assert_eq!(r.package, "example-add-macro", "{name} package");
    }
    // The runtime helper the facade's `App::connect` reads surfaces the same value.
    assert_eq!(
        modal_rust::__private::runtime::package_from_inventory(),
        Some("example-add-macro")
    );
}

#[test]
fn secret_vol_probe_reads_env_and_does_volume_io() {
    let key = "MODAL_RUST_PROBE_UNITTEST_SECRET";
    std::env::set_var(key, "hello-unit");
    let dir = std::env::temp_dir().join("modal_rust_probe_unittest");
    let _ = std::fs::remove_dir_all(&dir);
    let marker = dir.join("marker");
    let marker_path = marker.to_string_lossy().to_string();

    let first = secret_vol_probe(ProbeInput {
        secret_key: key.to_string(),
        marker_path: marker_path.clone(),
        write_value: Some("persisted-value".to_string()),
    })
    .unwrap();
    assert_eq!(first.secret_value.as_deref(), Some("hello-unit"));
    assert!(first.wrote);
    assert_eq!(first.marker_read.as_deref(), Some("persisted-value"));

    let second = secret_vol_probe(ProbeInput {
        secret_key: key.to_string(),
        marker_path: marker_path.clone(),
        write_value: None,
    })
    .unwrap();
    assert!(!second.wrote);
    assert_eq!(second.marker_read.as_deref(), Some("persisted-value"));

    std::env::remove_var(key);
    let _ = std::fs::remove_dir_all(&dir);
}
