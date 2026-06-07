//! Image-builder steps, proven OFFLINE.
//!
//! The `#[modal_rust::function] fn render` from this crate's `lib.rs` is a plain Rust
//! fn. The lesson is the IMAGE it would be built in — to which you add arbitrary
//! system / Python dependencies through the BUILD config, not the decorator:
//!
//! - [`ImageStep::apt`] (Modal `apt_install`) — system packages via `apt-get install`.
//! - [`ImageStep::pip`] (Modal `pip_install`) — Python packages via `pip install`.
//! - [`ImageStep::run`] (Modal `run_commands`) — arbitrary build-time shell commands.
//!
//! `App::dry_run` projects EXACTLY the request sequence `.remote()` would send, with NO
//! Modal and NO network — so we read the rendered image dockerfile straight off the
//! planned manifest and print the three step lines. The same projection is asserted by
//! `tests/manifest.rs`.
//!
//! Run: `cargo run -p example-pip-apt-image --bin pip_apt_image` (offline; no credentials).

use modal_rust::{App, ImageStep, PlannedRequest, RemoteConfig};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // The build config — the image-builder STEPS, chained in order. The same effect a
    // Modal Python user gets from `image.apt_install(...).pip_install(...).run_commands(...)`.
    // `use_cargo_scoping: false` keeps the projection from shelling out to
    // `cargo metadata`, so this stays a pure, offline render.
    let cfg = RemoteConfig {
        package: "example-pip-apt-image".to_string(),
        use_cargo_scoping: false,
        image_steps: vec![
            ImageStep::apt(["libpng-dev", "libjpeg-dev"]),
            ImageStep::pip(["numpy", "pillow"]),
            ImageStep::run(["echo built > /opt/marker"]),
        ],
        ..RemoteConfig::default()
    };

    // `App::local()` builds the app from the `#[modal_rust::function]` inventory with
    // ZERO Modal. `dry_run` projects the RUN manifest `.remote()` WOULD send.
    let app = App::local();
    let manifest = app.dry_run("render", &cfg)?;

    // The base/run image is layer 0. Its `dockerfile_commands` are exactly what the
    // wire carries — so the apt/pip/run step lines are visible.
    let dockerfile = manifest
        .requests
        .iter()
        .find_map(|r| match r {
            PlannedRequest::ImageGetOrCreate {
                dockerfile_commands,
                layer: 0,
            } => Some(dockerfile_commands.clone()),
            _ => None,
        })
        .expect("the RUN manifest plans a base image (layer 0)");

    let find = |needle: &str| {
        dockerfile
            .iter()
            .find(|c| c.contains(needle))
            .cloned()
            .unwrap_or_else(|| panic!("image dockerfile missing a step containing {needle:?}"))
    };
    let apt = find("apt-get install");
    let pip = find("pip install");
    let run = find("echo built");

    println!("apt: {apt}");
    println!("pip: {pip}");
    println!("run: {run}");

    // Document the contract this example guarantees: the three image-builder steps ride
    // into the rendered image dockerfile, in the order chained.
    assert!(
        apt.contains("libpng-dev") && apt.contains("libjpeg-dev"),
        "apt_install renders the system packages"
    );
    assert!(
        pip.contains("numpy") && pip.contains("pillow"),
        "pip_install renders the Python packages"
    );
    assert!(
        run == "RUN echo built > /opt/marker",
        "run_commands renders verbatim"
    );

    // Chain order is preserved: apt < pip < run.
    let pos = |needle: &str| dockerfile.iter().position(|c| c.contains(needle)).unwrap();
    assert!(
        pos("apt-get install") < pos("pip install") && pos("pip install") < pos("echo built"),
        "image-builder steps render in chain order (apt < pip < run)"
    );

    Ok(())
}
