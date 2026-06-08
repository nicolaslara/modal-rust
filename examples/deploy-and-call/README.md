# deploy-and-call

The run-vs-deploy build boundary: `.remote()` uploads source and runs `cargo
build` inside the function body on every cold start; `deploy` runs `cargo build
--release` **once** at image-build time and bakes the binary into the image so
each `call` invokes the prebuilt binary with no rebuild. `fib(n)` is the
deployed function; `src/bin/deploy_and_call.rs` is the offline contrast driver.

## Run it

Run ephemerally (builds in the function body, no deploy needed):

```bash
cd examples/deploy-and-call
modal-rust run fib --input '{"n":10}'
```

Expected output:

```json
{"ok":true,"value":55}
```

The intended production flow — build once, call many times:

```bash
modal-rust deploy fib --app deploy-and-call
modal-rust call fib --app deploy-and-call --input '{"n":10}'
```

Expected output:

```json
{"ok":true,"value":55}
```

Prove the boundary **offline** (no Modal credentials needed):

```bash
cargo run -p example-deploy-and-call --bin deploy_and_call
```

Expected output:

```
run:    image builds the binary? false  (=> .remote() runs cargo build IN the body)
deploy: image builds the binary? true   (=> the binary is baked ONCE at image-build time)
deploy: FunctionCreate mounts = 1 (client only), published = "deployed"
boundary: deploy builds ONCE at image-build, call invokes with no rebuild
```

## Prereqs

Modal credentials configured (`modal token new`). Run `modal-rust doctor --rust`
to check your environment first.
