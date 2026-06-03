"""modal-rust prototype call shim (the `call` path) — M8.

Calls the deployed Function on the PERSISTENT app "modal-rust-add-poc" via a
cross-app lookup (boundaries.md §5; tasks.md M8). This is the consumer side of the
deploy invariant: it never builds anything, never mounts source, and never runs
cargo — it simply invokes the already-deployed `call_entrypoint`, whose body execs
ONLY the prebuilt `/app/modal_runner`.

Flag-mapping (tasks.md, authoritative):
  `modal run` auto-binds CLI flags ONLY to a `@app.local_entrypoint()` by parameter
  name. So `main(entrypoint, input_json)` is the flag-bound driver; it forwards the
  args to the deployed Function via `Function.from_name(...).remote(...)`.

This proves the `from_name`/`.remote()` cross-app arg path (a DIFFERENT code path
from the fresh-`modal run` arg-routing of M1) and, for M8, that the deployed result
is stable until an explicit redeploy: editing local source does NOT change the
call result; only a redeploy does.
"""

import modal

# A local app object so `modal run` can host the local_entrypoint. The actual work
# lives on the SEPARATE persistent deployed app "modal-rust-add-poc", reached by
# name below — this `call_app` app deploys/builds nothing.
app = modal.App("modal-rust-call")


@app.local_entrypoint()
def main(entrypoint: str = "add", input_json: str = '{"a":40,"b":2}'):
    print(
        modal.Function.from_name("modal-rust-add-poc", "call_entrypoint").remote(
            entrypoint, input_json
        )
    )
