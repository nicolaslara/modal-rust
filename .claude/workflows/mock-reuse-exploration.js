export const meta = {
  name: 'mock-reuse-exploration',
  description: 'Answer: can modal-rust reuse the PYTHON modal client mock (point our gRPC client at it) for offline tests, or is the Python mock an in-test pytest artifact configured per-test (so we are better off writing our own flexible Rust testing artifact)? Investigate the Python mock mechanics + the Rust-side feasibility, then recommend. Appends findings to docs/testing-strategy.md (do NOT commit/push — user keeps that doc uncommitted).',
  phases: [
    { title: 'Investigate', detail: 'parallel: (A) dissect the Python MockClientServicer/conftest/grpc_testing mechanics; (B) assess the Rust-side reuse cost vs writing our own' },
    { title: 'Synthesize', detail: 'answer full-server-vs-per-test-artifact + harder-than-our-own; recommend; append a section to docs/testing-strategy.md (no commit)' },
  ],
}

const ROOT = '/Users/nicolas/devel/modal-rust'

const SHARED = [
  'Repo root: ' + ROOT + '. This is an EXPLORATION (read + reason), not a code change. The question comes from the user:',
  '',
  'The offline-testing design doc (docs/testing-strategy.md, ALREADY WRITTEN, uncommitted) found that the Python modal',
  'client tests requests with an in-process MockClientServicer (references/modal-client test/conftest.py ~:625) + a',
  'shipped grpc_testing interceptor. The user\'s idea: modal-rust speaks the SAME gRPC wire (it is gRPC-compliant), so we',
  'could potentially POINT modal-rust\'s client at the Python mock instead of writing our own mock. The user\'s precise',
  'question:',
  '  "Is that harder than writing our own, flexible mocking library? The question is whether the Python mock is a FULL',
  '   SERVER we can run (point our client at it), or an IN-TEST ARTIFACT configured differently for each test. If the',
  '   latter, it is likely better to write our own testing artifact."',
  '',
  '## Ground truth to read',
  '- references/modal-client — the Python modal client + its test harness. Specifically hunt down: the MockClientServicer',
  '  class (test/conftest.py), how it is instantiated + wired to a channel/server, the grpc_testing helper',
  '  (modal/_utils/grpc_testing.py or similar), and how individual tests configure per-test behavior (servicer attributes,',
  '  response queues, monkeypatching, `servicer.function_body`/overrides, `ctx.get_requests(...)`).',
  '- modal-rust side: crates/modal-rust-sdk/src/channel.rs (it already supports plain http:// — comment says "so a local',
  '  dev server works"), the SDK `from_config` / server_url injection point, build.rs (build_server(false) today; the',
  '  service IS defined in the proto), and docs/testing-strategy.md (the layered plan: RequestSink recorder + optional',
  '  feature-gated in-process mock tonic server).',
  '',
  '## The crux to resolve (do NOT hand-wave — cite file:line)',
  '1. Is MockClientServicer a STANDALONE gRPC SERVER — i.e. registered on a real `grpc.server()` bound to a port/uds,',
  '   runnable as its own out-of-process process that ANY wire-level gRPC client (including our Rust tonic client) could',
  '   dial? OR is it driven through grpc_testing\'s IN-PROCESS, in-memory channel (no socket), only reachable from inside',
  '   the Python test process?  (Check how the servicer is attached: `grpc.server`+`add_ModalClientServicer_to_server`+',
  '   `add_insecure_port` => real server;  `grpc_testing.server_from_dictionary` / a fake channel => in-process only.)',
  '2. HOW is per-test behavior configured? Is it set by mutating Python servicer state / registering Python callables /',
  '   monkeypatching BEFORE each test (i.e. the canned responses + the FunctionGetOutputs body are Python objects defined',
  '   per test), or could the same server serve many tests unconfigured? This decides whether reuse forces a Python test',
  '   harness into our Rust test loop.',
  '3. If (1) says it CAN be run as a real server: what is the minimum to stand it up out-of-process for our Rust tests',
  '   (spawn a python -m ... process in CI? how do Rust tests then configure per-test responses across the language',
  '   boundary — they cannot set Python attributes)? If it CANNOT: reuse is effectively impossible without porting.',
  '',
  '## How to return',
  'End with: "RESULT: <one-line>". Be concrete and evidence-backed; quote the decisive lines.',
].join('\n')

phase('Investigate')
const findings = await parallel([
  () => agent(SHARED + '\n\n' + [
    'YOUR TASK (A — Python mock mechanics). Read references/modal-client deeply: locate MockClientServicer (the file +',
    'class), how it is attached to a server/channel (grpc.server + add_insecure_port + add_*Servicer_to_server, OR',
    'grpc_testing in-memory, OR a custom fake channel), where the test fixture lives (conftest), and how a representative',
    'handful of tests configure per-test behavior (servicer attributes, response dicts/queues, `function_body`, monkeypatch,',
    'ctx.get_requests). ANSWER crux #1 and #2 with exact file:line citations + quoted code. State definitively: FULL',
    'RUNNABLE SERVER (out-of-process, wire-reachable) vs IN-PROCESS PER-TEST ARTIFACT (and how per-test config works).',
    'RESULT: <SERVER|IN-PROCESS-ARTIFACT> — <one-line with the decisive citation>',
  ].join('\n'), { phase: 'Investigate', label: 'investigate:python-mock' }),

  () => agent(SHARED + '\n\n' + [
    'YOUR TASK (B — Rust-side reuse cost vs our own). Read crates/modal-rust-sdk/src/channel.rs + the from_config/',
    'server_url injection + build.rs, and docs/testing-strategy.md. Assess, concretely: (a) IF the Python mock can run as a',
    'real out-of-process server, what would it take for modal-rust tests to use it — spawning a Python process in CI, the',
    'cross-language per-test response-configuration problem (Rust cannot set Python servicer state), determinism,',
    'hermeticity, the extra CI dependency on a Python env + the modal package; (b) the cost of our OWN Rust artifact',
    'instead (the doc\'s RequestSink/RecordingSink recorder for whole-manifest assertions with NO server, AND/OR a',
    'feature-gated in-process tonic mock via build_server(true)). Compare on: effort, per-test flexibility, no-Python-in-CI,',
    'speed, maintenance. Note that gRPC-compliance gives WIRE compat but NOT test-harness reuse if responses are configured',
    'in Python per test. RESULT: <one-line recommendation with the main tradeoff>',
  ].join('\n'), { phase: 'Investigate', label: 'investigate:rust-cost' }),
])

phase('Synthesize')
const synth = await agent(SHARED + '\n\n' + [
  'Findings from the two investigators (verbatim):',
  '--- A (python-mock mechanics) ---', String(findings[0] || '(none)'),
  '--- B (rust-side cost) ---', String(findings[1] || '(none)'),
  '',
  'YOUR TASK (Synthesize + write). Directly answer the user\'s question: (1) Is the Python mock a FULL runnable server or',
  'an in-test per-test artifact? (2) Is reusing it HARDER than writing our own flexible Rust testing artifact? Give a',
  'clear RECOMMENDATION (reuse-python-mock vs build-our-own vs hybrid) with the 2-3 decisive reasons + the key tradeoff,',
  'and a concrete next step. APPEND a new section to ' + ROOT + '/docs/testing-strategy.md titled',
  '"## Reusing the Python mock vs building our own (investigation <leave date blank>)" with the evidence (file:line) and',
  'the recommendation. DO NOT commit or push — the user keeps docs/testing-strategy.md uncommitted. Confirm the file was',
  'appended. RESULT: <one-line: the verdict + recommendation>',
].join('\n'), { phase: 'Synthesize', label: 'synthesize:recommendation' })

return { findings, synth }
