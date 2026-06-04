export const meta = {
  name: 'harden-image-upload',
  description: 'Robustness pass: (A) IMAGE — do what the official modal client does: add_python (hosted python-standalone mount) + client mount, removing the apt+pip+python-is-python3+--break-system-packages hacks. (B) UPLOAD — scope to cargo metadata (only the crates in the dependency closure) + ignore resolution .modalignore > .gitignore > defaults; non-source extras go via volumes, not the source upload.',
  phases: [
    { title: 'Design', detail: 'read modal-client add_python/python-standalone + cargo metadata + the ignore crate -> one spec' },
    { title: 'Image', detail: 'add_python via python-standalone mount; drop the 3 image hacks (run + deploy); gates green (HARD GATE)' },
    { title: 'Upload', detail: 'cargo-metadata scoping + .modalignore>.gitignore>defaults ignore resolution; gates green (HARD GATE)' },
    { title: 'Live', detail: 're-prove .remote() + deploy on the add_python image (no hacks, faster); upload uploads only the dep-closure crates' },
    { title: 'Review', detail: 'parallel: image matches modal client (no hacks) + upload correct (cargo-scoped, ignore precedence) + hygiene' },
  ],
}

const ROOT = '/Users/nicolas/devel/modal-rust'

const SHARED = [
  'You are hardening the image + source-upload of modal-rust (repo root: ' + ROOT + '; git on main).',
  '',
  '## Where we are',
  'The run/deploy/call triad is proven live via our own first-party SDK (crates/modal-rust-sdk, no modal-rs dep) +',
  'the facade (crates/modal-rust). .local() (in-process), .remote() (run: build-in-body, EPHEMERAL app), and',
  'App::deploy/call (build-at-image-time, persistent) all return {sum:42}. retry_transient wraps every unary RPC.',
  'This workflow does the ROBUSTNESS PASS the user asked for — two independent hardening tracks.',
  '',
  '## TRACK A — IMAGE: "do the same thing the modal client does" (user directive)',
  'Today the run + deploy images use a CRUDE Python stack on a rust:slim base: `apt-get install python3 python3-pip',
  'python-is-python3` + `pip install --break-system-packages modal`. These are THREE hacks, all symptoms of NOT',
  'provisioning Python the way the official client does:',
  '  - `python-is-python3`: Modal init execs bare `python`; Debian apt gives only `python3` (crates/modal-rust/src/',
  '    remote.rs:252, deploy.rs:171).',
  '  - `--break-system-packages`: Debian system Python is PEP-668 externally-managed; pip refuses without it',
  '    (crates/modal-rust-sdk/src/ops/image.rs:184).',
  '  - `pip install modal`: provisions the client dep closure (typing_extensions etc.) the client mount does NOT carry.',
  'FIX: replicate the official client — use **add_python** (Modal\'s HOSTED python-standalone mount, resolved by name',
  'like the client mount — NO apt, NO build step) + the client mount. A python-standalone build HAS a `python` and is',
  'NOT externally-managed, so it dissolves python-is-python3 AND --break-system-packages. Determine EXACTLY how Modal',
  'provisions the client + its deps when add_python is used (does the standalone carry the deps? does Modal pip-install',
  'them against the standalone python? a deps mount?) and replicate that faithfully — read the references below. Apply',
  'to BOTH the run image (remote.rs) and the deploy image (deploy.rs). The image build should become near-instant (no',
  'apt/pip layers -> a short ImageJoinStreaming stream -> far fewer transport resets). Keep the apt+pip path available',
  'as a documented FALLBACK (e.g. behind a config flag) only if add_python\'s deps story has a gap — but the DEFAULT',
  'must be add_python.',
  '',
  '## TRACK B — UPLOAD: cargo-metadata scoping + ignore resolution (user directive)',
  'Today the source upload (crates/modal-rust-sdk/src/ops/local_dir.rs + crates/modal-rust/src/remote.rs RemoteConfig)',
  'uploads the WHOLE workspace root minus a HARDCODED ignore list ([target, .git, .modal-rust, *.rlib, references,',
  'workpads, .github, .claude, .cursor, .opencode, tmp, .research]) with a tiny non-gitignore matcher. That hardcoded',
  'list is brittle (the `references/` bug). Per the user, fix it TWO ways (cargo-metadata is the PRIMARY/preferred):',
  '  1. **cargo metadata scoping (PRIMARY):** shell out to `cargo metadata --format-version 1 --no-deps` (and/or with',
  '     resolve) at the workspace root; from it determine the workspace member crates IN THE DEPENDENCY CLOSURE of the',
  '     target package (the package being run/deployed, e.g. example-add -> + its path-dep members like',
  '     modal-rust-runtime). Upload ONLY those crate directories + the workspace Cargo.toml/Cargo.lock — NOT the whole',
  '     tree. This is correct-by-construction (cargo build needs exactly the closure) and robust (no hand-maintained',
  '     ignore list). Non-source extras (data, models) are the user\'s job to attach via VOLUMES, NOT the source upload.',
  '  2. **ignore-file resolution (pruning within the uploaded crates):** resolve ignore patterns from `.modalignore`',
  '     (HIGHEST precedence, if present) then `.gitignore` (if present) then the built-in defaults. Use a real',
  '     gitignore engine (the `ignore` crate from ripgrep handles .gitignore + custom ignore-file names cleanly).',
  '  Combine them: cargo-metadata picks WHICH crate dirs to upload; the ignore files prune WITHIN them (target/, etc.).',
  '  Keep a sensible fallback when cargo metadata is unavailable (no Cargo.toml / non-cargo project): fall back to the',
  '  workspace-root-minus-ignore behavior. Make the ignore-file names + the scoping behavior documented.',
  '',
  '## Ground-truth references (READ; never depend on — references/ is gitignored)',
  '- references/modal-client/py/modal/image.py — `add_python` (the from_registry/debian_slim add_python param: how it',
  '  attaches the python-standalone mount AND how the modal client + its deps get into the image).',
  '- references/modal-client/py/modal/mount.py — `python_standalone_mount_name` (~line 73), `python_standalone_mount`,',
  '  `_get_client_mount`/`_create_client_mount` (~696-734), `PYTHON_STANDALONE_VERSIONS`.',
  '- references/modal-rs/crates/modal-rs/src/{image.rs,mount.rs} — any add_python / python-standalone precedent.',
  '- crates/modal-rust-sdk/src/ops/{image.rs (with_apt/with_pip_install_modal/dockerfile_commands/context_mount),',
  '  mount.rs (client-mount resolution — the pattern to mirror for the python-standalone mount), local_dir.rs (upload',
  '  walker + IgnoreMatcher)}; crates/modal-rust/src/{remote.rs (run image + RemoteConfig + discover_local_root),',
  '  deploy.rs (deploy image)}.',
  '- For cargo metadata: `cargo metadata --format-version 1` JSON (workspace_members, packages[].{name,manifest_path},',
  '  resolve.nodes for the dep graph). Parse with serde_json (already a dep). The `ignore` crate (ripgrep) for',
  '  gitignore-style matching with a custom `.modalignore` filename.',
  '- workpads/shim-backend/knowledge.md (the 3 image findings + the upload-genericity gap).',
  '',
  '## FROZEN invariants — do NOT change',
  '- The runner CLI protocol, Registry/macros, the run-vs-deploy build boundary (run = build-in-body on mounted source;',
  '  deploy = build-at-image-time, runtime execs prebuilt binary). retry_transient stays on all RPCs. Do NOT rewrite',
  '  the proven create/invoke/deploy logic; this swaps the Python-provisioning mechanism + the upload file-selection.',
  '',
  '## Verification rules (WORKING.md)',
  '- Gates on default-members: cargo fmt --check ; cargo clippy --all-targets -- -D warnings ; cargo build ; cargo',
  '  test. Keep no-CUDA CI green. New deps (the `ignore` crate) minimal. Live tests behind #[ignore] + live feature.',
  '- Modal flakiness => RETRY. DRIVE live proofs to a terminal result (do not punt to a background monitor).',
  '- Use a STABLE app name for any live deploy test; do not leave persistent crash-loop-capable apps behind.',
  '',
  '## How to return',
  'End with: "RESULT: <STATUS> — <one-line>". Build phases STATUS in {BUILD_GREEN, BUILD_FAILED} + exact cargo output.',
  'Live: the decoded result + evidence (hacks gone / image faster / upload scoped), or the precise error after retries.',
].join('\n')

phase('Design')
const design = await parallel([
  () => agent(SHARED + '\n\n' + [
    'YOUR TASK (Design / TRACK A image — add_python the way modal does it). Read references/modal-client/py/modal/',
    'image.py (add_python) + mount.py (python_standalone_mount_name/python_standalone_mount/_get_client_mount/',
    '_create_client_mount/PYTHON_STANDALONE_VERSIONS) + references/modal-rs image.rs/mount.rs + our ops/{image.rs,',
    'mount.rs} + remote.rs/deploy.rs (the apt+pip hacks). Produce a PRECISE spec for:',
    '- How to provision Python via the HOSTED python-standalone mount (resolve `python-standalone-mount-{version}-{libc}`',
    '  by name via MountGetOrCreate GLOBAL, like the client mount; the exact name format, version/libc selection, mount',
    '  path, and how it lands on PATH so `python` resolves) — replicating the official client.',
    '- EXACTLY how the modal client + its dependency closure (typing_extensions, synchronicity, grpclib, protobuf, …)',
    '  get into the image when add_python is used: does the standalone carry them? does Modal pip-install them against',
    '  the standalone python (pip-friendly, no --break-system-packages, `python` present)? a separate deps mount? State',
    '  the verified mechanism + how WE replicate it for the run + deploy images so the 3 hacks (python-is-python3,',
    '  --break-system-packages, the bare apt+pip) are removed and the build has no slow apt/pip layer.',
    '- The additive ImageSpec changes (e.g. with_add_python / a python-standalone mount_id attached like the client',
    '  mount) and how run.rs + deploy.rs adopt it. Keep apt+pip as a documented fallback flag only.',
    'Cite file:line. RESULT: SPEC_DONE — add_python image spec (matches modal client)',
  ].join('\n'), { phase: 'Design', label: 'design:image-addpython' }),

  () => agent(SHARED + '\n\n' + [
    'YOUR TASK (Design / TRACK B upload — cargo-metadata scoping + ignore resolution). Read crates/modal-rust-sdk/src/',
    'ops/local_dir.rs (walker + IgnoreMatcher) + crates/modal-rust/src/remote.rs (RemoteConfig + discover_local_root +',
    'the hardcoded ignore list) and consider `cargo metadata --format-version 1` JSON + the ripgrep `ignore` crate.',
    'Produce a PRECISE spec for:',
    '- cargo-metadata SCOPING (primary): run `cargo metadata` at the workspace root, find the target package (the',
    '  package being run/deployed — derive from the existing PACKAGE/RemoteConfig), compute its WORKSPACE-MEMBER',
    '  dependency closure (path deps only; crates.io deps are fetched by cargo on Modal), and upload ONLY those crate',
    '  directories + the workspace Cargo.toml + Cargo.lock. Specify the JSON fields used (workspace_members, packages[]',
    '  .{id,name,manifest_path,dependencies}, resolve.nodes) and the closure algorithm. Fallback when cargo metadata',
    '  is unavailable: the current workspace-root-minus-ignore behavior.',
    '- IGNORE-FILE resolution (pruning within the uploaded dirs): .modalignore (highest precedence) > .gitignore (if',
    '  present) > built-in defaults, using the `ignore` crate (gitignore semantics + a custom .modalignore filename).',
    '  How it composes with the cargo scoping.',
    '- The RemoteConfig/API surface changes (keep it minimal; the common add case still works with zero config) + the',
    '  doc note that non-source extras (data/models) are attached via VOLUMES, not the source upload.',
    'Cite file:line + the exact cargo metadata fields. RESULT: SPEC_DONE — cargo-metadata + ignore-resolution upload spec',
  ].join('\n'), { phase: 'Design', label: 'design:upload-scoping' }),
])

const spec = await agent(SHARED + '\n\n' + [
  'YOUR TASK (Synthesize). Merge the two notes into ONE build-ready spec and WRITE it to',
  ROOT + '/workpads/shim-backend/harden-build-spec.md (overwrite if present): the add_python image mechanism (matching',
  'the modal client) for run + deploy, and the cargo-metadata-scoped upload + .modalignore>.gitignore>defaults ignore',
  'resolution. Note exactly which files change and the new deps (the `ignore` crate). Preserve the run-vs-deploy build',
  'boundary + retry_transient. Resolve contradictions (prefer what the official client actually does). Keep it tight.',
  '',
  '=== TRACK A (add_python IMAGE) NOTE ===',
  (design[0] || '(missing)'),
  '',
  '=== TRACK B (cargo-metadata + ignore UPLOAD) NOTE ===',
  (design[1] || '(missing)'),
  '',
  'RESULT: SPEC_DONE — wrote harden-build-spec.md',
].join('\n'), { phase: 'Design', label: 'design:synthesize' })

phase('Image')
const image = await agent(SHARED + '\n\n' + [
  'The spec is at ' + ROOT + '/workpads/shim-backend/harden-build-spec.md — READ IT FIRST.',
  '',
  'YOUR TASK (TRACK A — add_python image — HARD GATE). Implement per the spec: provision Python via the HOSTED',
  'python-standalone mount (resolved by name like the client mount) + the client mount, for BOTH the run image',
  '(remote.rs) and the deploy image (deploy.rs). Remove the three hacks (python-is-python3, --break-system-packages,',
  'the bare apt+pip) from the DEFAULT path; keep apt+pip behind a documented fallback flag only. Provision the modal',
  'client dep closure the way the official client does (per the spec). Add the additive ImageSpec support (e.g.',
  'with_add_python / python-standalone mount_id).',
  'Do NOT change the run-vs-deploy boundary or rewrite the proven create/invoke/deploy logic.',
  'VERIFY (offline hard): cargo fmt --check ; cargo clippy --all-targets -- -D warnings ; cargo build ; cargo test',
  '(all default-members) — all green. Paste exact output.',
  'RESULT: BUILD_GREEN — add_python image (run + deploy), 3 hacks removed from default path   (or BUILD_FAILED — <reason>)',
].join('\n'), { phase: 'Image', label: 'add-python-image' })

const imageGreen = /RESULT:\s*BUILD_GREEN/i.test(image || '')
let upload = null, live = null
if (!imageGreen) {
  log('Image HARD GATE not green — add_python image did not compile. Skipping Upload+Live; Review documents the blocker.')
} else {
  phase('Upload')
  upload = await agent(SHARED + '\n\n' + [
    'The spec is at ' + ROOT + '/workpads/shim-backend/harden-build-spec.md. The add_python image is implemented. First',
    'run cargo build and read the current local_dir.rs + remote.rs to orient.',
    '',
    'YOUR TASK (TRACK B — cargo-metadata scoping + ignore resolution — HARD GATE). Implement per the spec:',
    '- cargo-metadata scoping: upload ONLY the target package\'s workspace-member dependency-closure crate dirs +',
    '  workspace Cargo.toml/Cargo.lock (parse `cargo metadata --format-version 1` with serde_json). Fallback to the',
    '  current workspace-root-minus-ignore behavior when cargo metadata is unavailable.',
    '- ignore resolution: .modalignore (highest) > .gitignore > built-in defaults, via the ripgrep `ignore` crate',
    '  (add it as a minimal dep). Remove the brittle hardcoded list as the primary mechanism (keep defaults as fallback).',
    'Keep the common `add` case working with zero config. Add tests (a temp dir with .modalignore/.gitignore; a',
    'cargo-metadata closure test).',
    'VERIFY (offline hard): cargo fmt --check ; cargo clippy --all-targets -- -D warnings ; cargo build ; cargo test',
    '(all default-members) — all green. Paste exact output.',
    'RESULT: BUILD_GREEN — cargo-metadata-scoped upload + .modalignore/.gitignore resolution   (or BUILD_FAILED — <reason>)',
  ].join('\n'), { phase: 'Upload', label: 'cargo-scoped-upload' })

  const uploadGreen = /RESULT:\s*BUILD_GREEN/i.test(upload || '')
  if (!uploadGreen) {
    log('Upload HARD GATE not green. Skipping Live; Review documents the blocker.')
  } else {
    phase('Live')
    live = await agent(SHARED + '\n\n' + [
      'TRACK A (add_python image) + TRACK B (cargo-scoped upload) are implemented and compile. Run the LIVE proofs and',
      'DRIVE THEM TO TERMINAL RESULTS yourself (do not punt to a background monitor).',
      '',
      'YOUR TASK (LIVE re-prove on the hardened path). Against REAL Modal (live tests behind #[ignore]+live feature):',
      '  1. .remote() run: app.function("add").remote(AddInput{40,2}).await? == {sum:42} on the add_python image — NO',
      '     python-is-python3 / --break-system-packages / bare pip in the image; the container boots (python resolves,',
      '     modal client imports) and cargo builds in-body. Note the image-build time vs the old apt+pip path.',
      '  2. deploy + call: App::deploy(stable name) + call("add",{40,2}) == {sum:42} on the add_python deploy image,',
      '     cargo at image-build time, absent at call.',
      '  3. upload scoping: confirm the source upload now ships ONLY the dep-closure crates (not the whole tree) — e.g.',
      '     log/inspect the uploaded file set or mount size; and that a .modalignore is honored.',
      'Modal flakiness => RETRY. If a real bug surfaces, make the MINIMAL fix + re-verify offline gates.',
      'Capture: both {sum:42} results; evidence the 3 hacks are gone and the build is faster; the scoped upload set.',
      'RESULT: BUILD_GREEN — add_python run+deploy live == {sum:42} (no hacks), upload cargo-scoped   (or BUILD_FAILED/INFRA_BLOCKED — <detail>)',
    ].join('\n'), { phase: 'Live', label: 'live-hardened' })
  }
}

phase('Review')
const reviews = await parallel([
  () => agent(SHARED + '\n\n' + [
    'YOUR TASK (Review / image matches the modal client). Verify against references/modal-client/py/modal/{image.py,',
    'mount.py} + the code: Python is provisioned via the hosted python-standalone mount (resolved by name, attached',
    'like the client mount) the way the official client does; the THREE hacks (python-is-python3, --break-system-packages,',
    'bare apt+pip) are GONE from the default run + deploy images (apt+pip only as a documented fallback flag, if kept);',
    'the client + its dep closure are provisioned correctly so the container boots (no ModuleNotFoundError). Quote the',
    'relevant code. RESULT: PASS — image matches modal client, hacks removed  (or FAIL — <what is wrong>)',
  ].join('\n'), { phase: 'Review', label: 'review:image' }),

  () => agent(SHARED + '\n\n' + [
    'YOUR TASK (Review / upload correctness). Verify: the source upload is scoped via cargo metadata to the target',
    'package\'s workspace-member dependency closure + workspace manifests (NOT the whole tree, NOT a hardcoded ignore',
    'list as the primary mechanism); ignore resolution is .modalignore > .gitignore > defaults via the `ignore` crate;',
    'there is a sane fallback for non-cargo dirs; and `cargo build -p <target>` would still succeed on Modal with the',
    'uploaded set (the closure is complete: target + its path-dep members + Cargo.toml/lock). Quote the code + a test.',
    'RESULT: PASS — upload cargo-scoped + ignore precedence correct  (or FAIL — <what is wrong>)',
  ].join('\n'), { phase: 'Review', label: 'review:upload' }),

  () => agent(SHARED + '\n\n' + [
    'YOUR TASK (Review / hygiene — RUN the gates). From ' + ROOT + ' report exact output + exit status:',
    '- cargo fmt --check ; cargo clippy --all-targets -- -D warnings ; cargo build ; cargo test  (default-members).',
    'Confirm example-burn-add still excluded from default-members; live tests #[ignore]+live gated; the new `ignore`',
    'dep is minimal; no hand-written file grossly exceeds ~500 LOC. Report failures verbatim.',
    'RESULT: PASS — gates green  (or FAIL — <exact failing command + output>)',
  ].join('\n'), { phase: 'Review', label: 'review:hygiene' }),
])

return { image_green: imageGreen, image, upload, live, reviews }
