//! The runner binary for the pip-apt-image example — the WHOLE thing, one line.
//!
//! `modal_rust::modal_runner!(<lib>)` expands to the runner `main()`: it links the
//! library crate's `#[modal_rust::function]` `inventory` submissions, assembles the
//! registry + decorator configs, and runs the FROZEN runner CLI protocol (prints
//! exactly one JSON envelope to stdout, mirrors `ok` in the exit code). The image
//! build steps (apt/pip/run) are BUILD-path config (RemoteConfig.image_steps), not
//! decorator config, so `--describe` shows only the entrypoint config; the rendered
//! IMAGE dockerfile is proven OFFLINE by the `pip_apt_image` driver + `tests/manifest.rs`.
//! The user never writes `main()`.

modal_rust::modal_runner!(example_pip_apt_image);
