//! `burn-lm-bench` — a tiny black-box load test for the `burn-lm-http`
//! inference server.
//!
//! It hits the OpenAI-compatible `POST {url}/v1/chat/completions` endpoint with
//! a closed-loop set of concurrent requests (exactly `--concurrency` in flight
//! until `--requests` complete) and prints throughput / latency stats.
//!
//! No `burn` dependencies: this is a pure HTTP client. Percentiles are
//! hand-rolled; request errors are counted, never fatal.

use clap::Parser;
use futures_util::StreamExt;
use std::collections::HashSet;
use std::fs::OpenOptions;
use std::io::Write as _;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::Semaphore;

use plotly::common::{DashType, Line, Mode};
use plotly::layout::{Axis, Layout};
use plotly::{Plot, Scatter};

#[derive(Parser, Debug, Clone)]
#[command(about = "Concurrent HTTP load test for a burn-lm-http inference server")]
struct Args {
    /// Base URL of the server (no trailing path).
    #[arg(long, default_value = "http://localhost:3000")]
    url: String,
    /// Model name to request (required by the API in single-run mode).
    #[arg(long, default_value = "")]
    model: String,
    /// Number of requests kept in flight at once.
    #[arg(long, default_value_t = 8)]
    concurrency: usize,
    /// Total number of requests to send.
    #[arg(long, default_value_t = 64)]
    requests: usize,
    /// Max tokens to generate per request.
    #[arg(long, default_value_t = 256)]
    max_tokens: u64,
    /// Prompt sent as the user message.
    #[arg(long, default_value = "Write a short paragraph about the ocean.")]
    prompt: String,
    /// Use server-sent-events streaming (records time-to-first-token).
    #[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
    stream: bool,
    /// In streaming mode, deliberately drop each response after this many non-empty
    /// content chunks. 0 drains streams normally.
    #[arg(long, default_value_t = 0)]
    disconnect_after_tokens: u64,
    /// Also print the summary as a single JSON line.
    #[arg(long, default_value_t = false)]
    json: bool,

    // --- sweep mode ---
    /// Run a model x concurrency sweep, then write results.csv + HTML charts.
    #[arg(long, default_value_t = false)]
    sweep: bool,
    /// Comma-separated model ids to sweep (overrides --model when set).
    #[arg(long, default_value = "")]
    models: String,
    /// Comma-separated concurrency levels for the sweep (ascending).
    #[arg(long, default_value = "1,2,4,8,16,32")]
    levels: String,
    /// Requests per sweep point = (concurrency * rounds), floored at 2*rounds. Lower this
    /// for slow/large models so each point finishes in reasonable wall time; higher for
    /// a steadier measurement. (Default keeps the batch busy for ~6 rounds.)
    #[arg(long, default_value_t = 6)]
    rounds: usize,
    /// Directory to write results.csv, results.jsonl, and the HTML charts into.
    /// Defaults under `.modal-rust/` (already gitignored repo-wide), so a sweep never
    /// litters the cwd or lands in a committed path. Override with `--out-dir` — e.g.
    /// a per-GPU subdir like `.modal-rust/burn-lm-bench-runs/t4`.
    #[arg(long, default_value = ".modal-rust/burn-lm-bench-runs")]
    out_dir: String,
    /// Skip running entirely; just (re)render the HTML charts from the existing
    /// results.csv (useful to chart partial results after a crash).
    #[arg(long, default_value_t = false)]
    plot_only: bool,
}

/// Aggregate metrics for one (model, concurrency) point.
#[derive(Clone, Debug)]
struct Summary {
    model: String,
    concurrency: usize,
    total: usize,
    ok: usize,
    failed: usize,
    aborted: usize,
    wall_s: f64,
    requests_per_s: f64,
    tokens_per_s: f64,
    e2e_ms_p50: f64,
    e2e_ms_p95: f64,
    ttft_ms_p50: f64,
    ttft_ms_p95: f64,
    /// Median per-request DECODE rate = (out_tokens-1) / (e2e - ttft): the steady-state
    /// generation speed AFTER the first token, with the queue + prefill wait excluded.
    /// Robust to the interleaved-prefill straggler tail (continuous batching runs at most
    /// one prompt prefill per round, so admitting a request adds a one-round bump to the
    /// lanes already decoding — see README "Reading the numbers"). It stays high even when
    /// ttft/e2e blow up, so it isolates "is decode itself fast?" from "did this request
    /// wait for an admission round?". `tokens_per_s` (aggregate) stays the headline.
    decode_tok_s_p50: f64,
    failure_rate: f64,
}

/// One request outcome. Times are durations in milliseconds.
#[derive(Clone)]
struct Outcome {
    ok: bool,
    e2e_ms: f64,
    ttft_ms: Option<f64>,
    out_tokens: u64,
    aborted: bool,
    /// True if `out_tokens` was estimated (chars/4) rather than from `usage`.
    /// Retained for diagnostics; not currently surfaced in the summary.
    #[allow(dead_code)]
    approx_tokens: bool,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    // --plot-only implies sweep handling (it only renders from results.csv).
    if args.sweep || args.plot_only {
        run_sweep(&args).await
    } else {
        run_single(&args).await
    }
}

/// Single-run mode: one (model, concurrency) point, printed (and optionally
/// emitted as a JSON line) exactly as before.
async fn run_single(args: &Args) -> Result<(), Box<dyn std::error::Error>> {
    if args.model.is_empty() {
        return Err("--model is required (or use --sweep --models ...)".into());
    }
    eprintln!(
        "burn-lm-bench: url={} model={} concurrency={} requests={} max_tokens={} stream={}",
        args.url, args.model, args.concurrency, args.requests, args.max_tokens, args.stream
    );
    if args.disconnect_after_tokens > 0 {
        eprintln!(
            "burn-lm-bench: deliberate disconnect after {} streamed content chunk(s)",
            args.disconnect_after_tokens
        );
    }

    let s = run_point(args, &args.model, args.concurrency, args.requests, None).await;

    let tok_label = ""; // approx-ness already folded into tokens_per_s
    println!("\n=== burn-lm-bench summary ===");
    println!(
        "total={}  ok={}  failed={}  aborted={}",
        s.total, s.ok, s.failed, s.aborted
    );
    println!("wall={:.2}s  requests/s={:.2}", s.wall_s, s.requests_per_s);
    println!(
        "output tokens/s={:.1}{tok_label}  (aggregate system throughput — the headline)",
        s.tokens_per_s
    );
    println!(
        "e2e latency ms: p50={:.1}  p95={:.1}",
        s.e2e_ms_p50, s.e2e_ms_p95
    );
    if args.stream {
        println!(
            "ttft ms:        p50={:.1}  p95={:.1}",
            s.ttft_ms_p50, s.ttft_ms_p95
        );
        println!(
            "decode tok/s:   p50={:.1}  (per-request, excludes ttft — robust to the prefill tail)",
            s.decode_tok_s_p50
        );
    }

    if args.json {
        let summary = serde_json::json!({
            "total": s.total,
            "ok": s.ok,
            "failed": s.failed,
            "aborted": s.aborted,
            "wall_s": s.wall_s,
            "requests_per_s": s.requests_per_s,
            "tokens_per_s": s.tokens_per_s,
            "e2e_ms_p50": s.e2e_ms_p50,
            "e2e_ms_p95": s.e2e_ms_p95,
            "ttft_ms_p50": if args.stream { Some(s.ttft_ms_p50) } else { None },
            "ttft_ms_p95": if args.stream { Some(s.ttft_ms_p95) } else { None },
            "decode_tok_s_p50": if args.stream { Some(s.decode_tok_s_p50) } else { None },
        });
        println!("{}", serde_json::to_string(&summary).unwrap());
    }
    Ok(())
}

/// A line-buffered, mutex-guarded appender used to salvage per-request results to
/// `results.jsonl` as each request finishes (so a point that dies halfway still
/// leaves its completed requests on disk).
type JsonlSink = Arc<Mutex<std::fs::File>>;

/// Run exactly one (model, concurrency) point and return its aggregate metrics.
/// Closed-loop concurrency: a semaphore caps in-flight requests; we spawn
/// exactly `requests` tasks but only `concurrency` run at any moment.
///
/// When `jsonl` is `Some`, each completed request is appended (and flushed) to it
/// as soon as it is collected, so partial work survives a mid-point crash.
/// Untimed model warm-up: issue ONE request and wait for it to finish so the
/// model's weights are downloaded and its CUDA kernels are compiled before any
/// timed point runs. A failure is logged, not fatal — a persistent problem then
/// shows up as the first timed point's 0-success row, which the sweep retries.
async fn warmup(url: &str, model: &str, prompt: &str) {
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .build()
        .expect("reqwest client");
    let endpoint = format!("{}/v1/chat/completions", url.trim_end_matches('/'));
    // STREAMING warmup on purpose: the cold load takes ~60-130s, during which a
    // non-streaming request sends no bytes and Modal's web proxy idle-times-out the
    // connection ("error sending request"). A streaming request emits the load-progress
    // banner as it goes, keeping the connection alive until the model is ready.
    let body = serde_json::json!({
        "model": model,
        "messages": [{"role": "user", "content": prompt}],
        "max_tokens": 8,
        "temperature": 0,
        "stream": true,
    });
    eprintln!("warmup {model}: loading (first request downloads weights / compiles kernels)...");
    // The HF weight download can drop with a connection reset (Modal <-> HuggingFace
    // flake), which panics the batching worker (WorkerDied) BEFORE the model is loaded.
    // A single warmup would then silently leave the model unloaded and every measured
    // request would fail (re-download churn) — confounding the whole sweep. So RETRY
    // until a request actually streams a generated token. The weights are cached on the
    // first success, so a retry that gets past the download is fast and stable.
    const MAX_ATTEMPTS: usize = 8;
    for attempt in 1..=MAX_ATTEMPTS {
        let t = Instant::now();
        match client.post(&endpoint).json(&body).send().await {
            Ok(resp) => {
                let status = resp.status();
                // Loaded == the stream actually produced a generated token ("content"
                // delta) or reached the clean end ("[DONE]"). A failed load yields a
                // non-2xx, or a stream that errors out before any content.
                let mut generated = false;
                let mut stream = resp.bytes_stream();
                while let Some(chunk) = stream.next().await {
                    match chunk {
                        Ok(bytes) => {
                            let s = String::from_utf8_lossy(&bytes);
                            if s.contains("\"content\"") || s.contains("[DONE]") {
                                generated = true;
                            }
                        }
                        Err(_) => break,
                    }
                }
                if status.is_success() && generated {
                    eprintln!(
                        "warmup {model}: ready in {:.1}s (HTTP {status}, attempt {attempt})",
                        t.elapsed().as_secs_f64()
                    );
                    return;
                }
                eprintln!(
                    "warmup {model}: attempt {attempt}/{MAX_ATTEMPTS} not ready \
                     (HTTP {status}, generated={generated}) after {:.1}s; retrying…",
                    t.elapsed().as_secs_f64()
                );
            }
            Err(e) => eprintln!(
                "warmup {model}: attempt {attempt}/{MAX_ATTEMPTS} request failed after {:.1}s ({e}); retrying…",
                t.elapsed().as_secs_f64()
            ),
        }
        if attempt < MAX_ATTEMPTS {
            tokio::time::sleep(Duration::from_secs(4)).await;
        }
    }
    eprintln!(
        "warmup {model}: NOT ready after {MAX_ATTEMPTS} attempts — the weight download keeps \
         resetting; measurements for this model will be unreliable"
    );
}

/// Warm the batch-`concurrency` kernels: cubecl JIT-compiles kernels per batch shape,
/// so the FIRST request seen at a new concurrency pays a one-time compile (tens of
/// seconds). Fire `concurrency` concurrent untimed requests so that compile happens
/// HERE, not inside the timed point. Outcomes are discarded.
async fn warm_level(url: &str, model: &str, prompt: &str, concurrency: usize) {
    let client = Arc::new(
        reqwest::Client::builder()
            .pool_max_idle_per_host(concurrency)
            .connect_timeout(Duration::from_secs(10))
            .build()
            .expect("reqwest client"),
    );
    let endpoint = Arc::new(format!("{}/v1/chat/completions", url.trim_end_matches('/')));
    let body = Arc::new(serde_json::json!({
        "model": model,
        "messages": [{"role": "user", "content": prompt}],
        "max_tokens": 8,
        "temperature": 0,
        "stream": true,
    }));
    let t = Instant::now();
    let mut tasks = Vec::with_capacity(concurrency);
    for _ in 0..concurrency {
        let (c, e, b) = (client.clone(), endpoint.clone(), body.clone());
        tasks.push(tokio::spawn(async move {
            one_request(&c, &e, &b, true, None).await
        }));
    }
    for t in tasks {
        let _ = t.await;
    }
    eprintln!(
        "warm-level c={concurrency}: batch kernels ready in {:.1}s",
        t.elapsed().as_secs_f64()
    );
}

async fn run_point(
    args: &Args,
    model: &str,
    concurrency: usize,
    requests: usize,
    jsonl: Option<&JsonlSink>,
) -> Summary {
    let client = reqwest::Client::builder()
        .pool_max_idle_per_host(concurrency)
        .connect_timeout(Duration::from_secs(10))
        .build()
        .expect("reqwest client");

    let endpoint = format!("{}/v1/chat/completions", args.url.trim_end_matches('/'));
    let body = serde_json::json!({
        "model": model,
        "messages": [{"role": "user", "content": args.prompt}],
        "max_tokens": args.max_tokens,
        "temperature": 0,
        "stream": args.stream,
    });

    let sem = Arc::new(Semaphore::new(concurrency));
    let client = Arc::new(client);
    let endpoint = Arc::new(endpoint);
    let body = Arc::new(body);

    let wall_start = Instant::now();
    let mut tasks = Vec::with_capacity(requests);
    for _ in 0..requests {
        let permit = sem.clone().acquire_owned().await.unwrap();
        let (client, endpoint, body, stream, disconnect_after_tokens) = (
            client.clone(),
            endpoint.clone(),
            body.clone(),
            args.stream,
            (args.disconnect_after_tokens > 0).then_some(args.disconnect_after_tokens),
        );
        tasks.push(tokio::spawn(async move {
            let _permit = permit; // released when the request finishes
            one_request_resilient(&client, &endpoint, &body, stream, disconnect_after_tokens).await
        }));
    }

    let mut outcomes = Vec::with_capacity(requests);
    for t in tasks {
        let o = match t.await {
            Ok(o) => o,
            // A panicked/cancelled request task counts as a failed outcome
            // rather than aborting the whole point.
            Err(_) => Outcome {
                ok: false,
                e2e_ms: 0.0,
                ttft_ms: None,
                out_tokens: 0,
                aborted: false,
                approx_tokens: false,
            },
        };
        // Salvage this request to results.jsonl immediately (incremental flush)
        // so partial points still leave their completed requests on disk.
        if let Some(sink) = jsonl {
            let line = serde_json::json!({
                "model": model,
                "concurrency": concurrency,
                "e2e_ms": o.e2e_ms,
                "ttft_ms": o.ttft_ms,
                "out_tokens": o.out_tokens,
                "ok": o.ok,
                "aborted": o.aborted,
            });
            if let Ok(mut f) = sink.lock() {
                let _ = writeln!(f, "{}", line);
                let _ = f.flush();
            }
        }
        outcomes.push(o);
    }
    let wall_s = wall_start.elapsed().as_secs_f64();
    aggregate(model, concurrency, &outcomes, wall_s)
}

/// Fold per-request outcomes into a `Summary` of aggregate metrics.
fn aggregate(model: &str, concurrency: usize, outcomes: &[Outcome], wall_s: f64) -> Summary {
    let total = outcomes.len();
    let ok: Vec<&Outcome> = outcomes.iter().filter(|o| o.ok).collect();
    let failed = total - ok.len();
    let aborted = outcomes.iter().filter(|o| o.aborted).count();

    let mut e2e: Vec<f64> = ok.iter().map(|o| o.e2e_ms).collect();
    e2e.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let mut ttft: Vec<f64> = ok.iter().filter_map(|o| o.ttft_ms).collect();
    ttft.sort_by(|a, b| a.partial_cmp(b).unwrap());

    // Per-request DECODE rate = (out_tokens-1) inter-token intervals / (e2e - ttft).
    // Needs streaming (ttft present), >=2 tokens, and e2e > ttft. This is the rate that
    // EXCLUDES the queue + prefill wait, so it stays flat under the straggler tail.
    let mut decode: Vec<f64> = ok
        .iter()
        .filter_map(|o| {
            let ttft = o.ttft_ms?;
            let decode_s = (o.e2e_ms - ttft) / 1000.0;
            (o.out_tokens >= 2 && decode_s > 0.0)
                .then(|| (o.out_tokens as f64 - 1.0) / decode_s)
        })
        .collect();
    decode.sort_by(|a, b| a.partial_cmp(b).unwrap());

    let out_tokens: u64 = ok.iter().map(|o| o.out_tokens).sum();
    let requests_per_s = if wall_s > 0.0 {
        total as f64 / wall_s
    } else {
        0.0
    };
    let tokens_per_s = if wall_s > 0.0 {
        out_tokens as f64 / wall_s
    } else {
        0.0
    };
    let failure_rate = if total > 0 {
        failed as f64 / total as f64
    } else {
        0.0
    };

    Summary {
        model: model.to_string(),
        concurrency,
        total,
        ok: ok.len(),
        failed,
        aborted,
        wall_s,
        requests_per_s,
        tokens_per_s,
        e2e_ms_p50: pct(&e2e, 50.0),
        e2e_ms_p95: pct(&e2e, 95.0),
        ttft_ms_p50: pct(&ttft, 50.0),
        ttft_ms_p95: pct(&ttft, 95.0),
        decode_tok_s_p50: pct(&decode, 50.0),
        failure_rate,
    }
}

/// Max attempts for a single timed request. Modal's web_server proxy occasionally
/// resets a long-lived streaming connection (`h2 protocol error` / `ConnectionReset`),
/// which otherwise lands as a hard failure and pollutes an entire concurrency point.
const MAX_REQUEST_ATTEMPTS: usize = 3;

/// [`one_request`] with a bounded retry for TRANSIENT connection failures only. A
/// request is retried solely when it produced no tokens AND wasn't a deliberate
/// disconnect (`!ok && !aborted && out_tokens == 0`) — i.e. the connection dropped
/// before any output. A successful, partial, or deliberately-aborted outcome is
/// returned as-is, so a slow-but-valid response is never masked or double-counted.
async fn one_request_resilient(
    client: &reqwest::Client,
    endpoint: &str,
    body: &serde_json::Value,
    stream: bool,
    disconnect_after_tokens: Option<u64>,
) -> Outcome {
    let mut last = one_request(client, endpoint, body, stream, disconnect_after_tokens).await;
    let mut attempts = 1;
    while attempts < MAX_REQUEST_ATTEMPTS && !last.ok && !last.aborted && last.out_tokens == 0 {
        tokio::time::sleep(Duration::from_millis(500)).await;
        last = one_request(client, endpoint, body, stream, disconnect_after_tokens).await;
        attempts += 1;
    }
    last
}

/// Issue a single chat-completion request and time it. Errors are captured as a
/// failed `Outcome` rather than propagated, so one bad request never aborts the
/// run.
async fn one_request(
    client: &reqwest::Client,
    endpoint: &str,
    body: &serde_json::Value,
    stream: bool,
    disconnect_after_tokens: Option<u64>,
) -> Outcome {
    let fail = |e2e: f64| Outcome {
        ok: false,
        e2e_ms: e2e,
        ttft_ms: None,
        out_tokens: 0,
        aborted: false,
        approx_tokens: false,
    };

    let start = Instant::now();
    let resp = match client.post(endpoint).json(body).send().await {
        Ok(r) => r,
        Err(_) => return fail(start.elapsed().as_secs_f64() * 1000.0),
    };
    if !resp.status().is_success() {
        return fail(start.elapsed().as_secs_f64() * 1000.0);
    }

    if stream {
        // Parse SSE: lines `data: {json}` carrying choices[0].delta.content,
        // terminated by `data: [DONE]`. Record the first-chunk time as TTFT.
        let mut ttft_ms = None;
        let mut text = String::new();
        let mut usage_tokens: Option<u64> = None;
        // burn-lm-http streams ONE token per SSE chunk and reports usage=null, so we
        // count non-empty content deltas as the output-token count — far more accurate
        // than chars/4 (a multi-char header chunk is 1 token, not ~5). TTFT is recorded
        // on the first chunk regardless.
        let mut delta_chunks: u64 = 0;
        let mut buf = String::new();
        let mut bytes = resp.bytes_stream();
        while let Some(chunk) = bytes.next().await {
            let chunk = match chunk {
                Ok(c) => c,
                Err(_) => return fail(start.elapsed().as_secs_f64() * 1000.0),
            };
            buf.push_str(&String::from_utf8_lossy(&chunk));
            // Process complete lines; keep any partial tail in `buf`.
            while let Some(nl) = buf.find('\n') {
                let line = buf[..nl].trim().to_string();
                buf.drain(..=nl);
                let data = match line.strip_prefix("data:") {
                    Some(d) => d.trim(),
                    None => continue,
                };
                if data == "[DONE]" {
                    break;
                }
                if ttft_ms.is_none() {
                    ttft_ms = Some(start.elapsed().as_secs_f64() * 1000.0);
                }
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(data) {
                    if let Some(c) = v["choices"][0]["delta"]["content"].as_str() {
                        if !c.is_empty() {
                            delta_chunks += 1;
                        }
                        text.push_str(c);
                    }
                    if let Some(n) = v["usage"]["completion_tokens"].as_u64() {
                        usage_tokens = Some(n);
                    }
                }
                if disconnect_after_tokens.is_some_and(|limit| limit > 0 && delta_chunks >= limit) {
                    let e2e_ms = start.elapsed().as_secs_f64() * 1000.0;
                    return Outcome {
                        ok: true,
                        e2e_ms,
                        ttft_ms,
                        out_tokens: delta_chunks,
                        aborted: true,
                        approx_tokens: true,
                    };
                }
            }
        }
        let e2e_ms = start.elapsed().as_secs_f64() * 1000.0;
        // Prefer a server-reported positive usage; otherwise the streamed-chunk count
        // (one token per chunk). chars/4 is the last resort if nothing streamed.
        let (out_tokens, approx) = match usage_tokens {
            Some(n) if n > 0 => (n, false),
            _ if delta_chunks > 0 => (delta_chunks, true),
            _ => ((text.chars().count() as u64) / 4, true),
        };
        Outcome {
            ok: true,
            e2e_ms,
            ttft_ms,
            out_tokens,
            aborted: false,
            approx_tokens: approx,
        }
    } else {
        // Single JSON response: choices[0].message.content + usage.
        let v = match resp.json::<serde_json::Value>().await {
            Ok(v) => v,
            Err(_) => return fail(start.elapsed().as_secs_f64() * 1000.0),
        };
        let e2e_ms = start.elapsed().as_secs_f64() * 1000.0;
        let (out_tokens, approx) = match v["usage"]["completion_tokens"].as_u64() {
            Some(n) => (n, false),
            None => {
                let text = v["choices"][0]["message"]["content"].as_str().unwrap_or("");
                ((text.chars().count() as u64) / 4, true)
            }
        };
        Outcome {
            ok: true,
            e2e_ms,
            ttft_ms: None,
            out_tokens,
            aborted: false,
            approx_tokens: approx,
        }
    }
}

/// Hand-rolled percentile (nearest-rank on a sorted slice). `p` in [0,100].
fn pct(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let rank = (p / 100.0 * (sorted.len() as f64 - 1.0)).round() as usize;
    sorted[rank.min(sorted.len() - 1)]
}

/// Sweep mode: model x concurrency -> results.csv + throughput/speedup/latency HTML charts.
async fn run_sweep(args: &Args) -> Result<(), Box<dyn std::error::Error>> {
    std::fs::create_dir_all(&args.out_dir)?;
    let out = Path::new(&args.out_dir);
    let csv_path = out.join("results.csv");

    // --plot-only: skip running (and even arg validation), just (re)render from
    // the existing results.csv. Lets you chart partial data after any crash.
    if args.plot_only {
        eprintln!("plot-only: rendering charts from {}", csv_path.display());
        render_charts(out, &csv_path)?;
        return Ok(());
    }

    let models: Vec<String> = if args.models.trim().is_empty() {
        if args.model.is_empty() {
            return Err("--sweep needs --models (comma-separated) or --model".into());
        }
        vec![args.model.clone()]
    } else {
        args.models
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    };

    let mut levels: Vec<usize> = args
        .levels
        .split(',')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(|s| s.parse::<usize>())
        .collect::<Result<_, _>>()
        .map_err(|e| format!("bad --levels: {e}"))?;
    levels.sort_unstable();
    levels.dedup();
    if levels.is_empty() {
        return Err("--levels produced no concurrency values".into());
    }

    eprintln!(
        "burn-lm-bench sweep: url={} models={:?} levels={:?} max_tokens={} stream={} disconnect_after_tokens={}",
        args.url, models, levels, args.max_tokens, args.stream, args.disconnect_after_tokens
    );

    // --- Resume: read already-completed (model, concurrency) pairs from CSV. ---
    let done = read_done_pairs(&csv_path);

    // --- Incremental CSV append (header only if the file is new/empty). ---
    let csv_exists_nonempty = std::fs::metadata(&csv_path)
        .map(|m| m.len() > 0)
        .unwrap_or(false);
    let mut csv = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&csv_path)?;
    if !csv_exists_nonempty {
        // RAW metrics live in the row so a partial CSV is self-sufficient.
        // `speedup` is left BLANK here and computed at render time from the CSV,
        // because it depends on the lowest-level row which may be written in a
        // different run (resume) — render-time keeps it always correct.
        writeln!(
            csv,
            "model,concurrency,tokens_per_s,speedup,requests_per_s,e2e_ms_p95,ttft_ms_p95,failure_rate,total,ok,failed,wall_s,aborted,ttft_ms_p50,decode_tok_s_p50"
        )?;
        csv.flush()?;
    }

    // --- Per-request JSONL salvage sink (append mode, flushed per line). ---
    let jsonl_path = out.join("results.jsonl");
    let jsonl: JsonlSink = Arc::new(Mutex::new(
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(&jsonl_path)?,
    ));

    for model in &models {
        // Skip the whole model (and its warm-up) if every level is already recorded.
        if levels.iter().all(|&c| done.contains(&(model.clone(), c))) {
            eprintln!("skip {model} (all levels already done)");
            continue;
        }
        // Warm up ONCE, untimed: the first request to a model downloads its weights
        // and JIT-compiles its CUDA kernels (can take minutes). Paying that here keeps
        // it OUT of the concurrency=1 baseline — otherwise the load cost folds into the
        // baseline's wall time, depressing its tokens/s and INFLATING every speedup
        // number. It also surfaces an OOM / load failure before any timed point runs.
        warmup(&args.url, model, &args.prompt).await;

        for &c in &levels {
            if done.contains(&(model.clone(), c)) {
                eprintln!("skip {model}@{c} (already done)");
                continue;
            }
            let requests = (c * args.rounds).max(args.rounds * 2);
            eprintln!(">>> model={model} concurrency={c} requests={requests}");

            // Compile the batch-`c` kernels BEFORE timing. cubecl JIT-compiles kernels
            // per batch shape, so the first request at a new concurrency otherwise pays a
            // one-time ~tens-of-seconds compile that would wreck this point's throughput.
            warm_level(&args.url, model, &args.prompt, c).await;

            // Per-point error isolation: run the point inside a spawned task so
            // an unexpected panic in the point becomes an Err here and the sweep
            // continues to the next point instead of aborting.
            let args_c = args.clone();
            let model_c = model.clone();
            let jsonl_c = jsonl.clone();
            let handle = tokio::spawn(async move {
                run_point(&args_c, &model_c, c, requests, Some(&jsonl_c)).await
            });
            let s = match handle.await {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("    !! point {model}@{c} aborted ({e}); continuing");
                    continue;
                }
            };

            eprintln!(
                "    tokens/s={:.1}  req/s={:.2}  e2e_p95={:.0}ms  fail={:.1}%  aborted={}",
                s.tokens_per_s,
                s.requests_per_s,
                s.e2e_ms_p95,
                s.failure_rate * 100.0,
                s.aborted
            );

            // A fully-failed point (zero successful requests) is treated as a
            // TRANSIENT failure: we DELIBERATELY do NOT write its CSV row, so a
            // rerun of the same command RETRIES it (its partial requests are
            // already salvaged to results.jsonl). Only points with >=1 success
            // are recorded as "done" and thus skipped on resume.
            if s.ok == 0 {
                eprintln!(
                    "    !! point {model}@{c} had 0 successes; not recording (will retry on rerun)"
                );
                continue;
            }

            // Append + flush this row IMMEDIATELY so a crash on the next point
            // keeps every prior point. `speedup` blank (computed at render).
            writeln!(
                csv,
                "{},{},{:.2},,{:.2},{:.1},{:.1},{:.4},{},{},{},{:.3},{},{:.1},{:.2}",
                csv_escape(&s.model),
                s.concurrency,
                s.tokens_per_s,
                s.requests_per_s,
                s.e2e_ms_p95,
                s.ttft_ms_p95,
                s.failure_rate,
                s.total,
                s.ok,
                s.failed,
                s.wall_s,
                s.aborted,
                s.ttft_ms_p50,
                s.decode_tok_s_p50,
            )?;
            csv.flush()?;
        }
    }

    // Render from whatever made it into results.csv (partial is fine).
    render_charts(out, &csv_path)?;
    Ok(())
}

/// One parsed CSV data row (RAW metrics; speedup is derived, not read).
struct Row {
    model: String,
    concurrency: usize,
    tokens_per_s: f64,
    e2e_ms_p95: f64,
    ttft_ms_p95: f64,
    /// 0.0 when absent (rows written before the decode-rate column existed).
    decode_tok_s_p50: f64,
}

/// Read the already-completed (model, concurrency) pairs from an existing
/// results.csv, so a resumed sweep skips points it has already recorded.
fn read_done_pairs(csv_path: &Path) -> HashSet<(String, usize)> {
    let mut done = HashSet::new();
    let Ok(text) = std::fs::read_to_string(csv_path) else {
        return done;
    };
    for r in parse_rows(&text) {
        done.insert((r.model, r.concurrency));
    }
    done
}

/// Parse results.csv into data rows, skipping the header and any malformed line.
/// Only the columns needed for charting/resume are extracted.
fn parse_rows(text: &str) -> Vec<Row> {
    let mut rows = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with("model,") {
            continue; // header or blank
        }
        let f: Vec<&str> = line.split(',').collect();
        // model,concurrency,tokens_per_s,speedup,requests_per_s,e2e_ms_p95,ttft_ms_p95,
        //   ...,aborted[,ttft_ms_p50,decode_tok_s_p50]
        if f.len() < 7 {
            continue;
        }
        let (Ok(concurrency), Ok(tokens_per_s), Ok(e2e_ms_p95), Ok(ttft_ms_p95)) = (
            f[1].trim().parse::<usize>(),
            f[2].trim().parse::<f64>(),
            f[5].trim().parse::<f64>(),
            f[6].trim().parse::<f64>(),
        ) else {
            continue;
        };
        // decode_tok_s_p50 is the last (15th) column; absent in pre-decode-rate CSVs.
        let decode_tok_s_p50 = f.get(14).and_then(|s| s.trim().parse::<f64>().ok()).unwrap_or(0.0);
        rows.push(Row {
            model: f[0].trim().to_string(),
            concurrency,
            tokens_per_s,
            e2e_ms_p95,
            ttft_ms_p95,
            decode_tok_s_p50,
        });
    }
    rows
}

/// Render throughput/speedup/latency interactive HTML charts from results.csv (the
/// source of truth, so partial data charts correctly). Safe to call after a crash
/// via --plot-only.
fn render_charts(out: &Path, csv_path: &Path) -> std::io::Result<()> {
    let text = std::fs::read_to_string(csv_path).unwrap_or_default();
    let rows = parse_rows(&text);
    if rows.is_empty() {
        eprintln!(
            ">>> no data in {} yet; skipping chart render",
            csv_path.display()
        );
        return Ok(());
    }
    let by_model = group_by_model(&rows);

    let throughput: Vec<(String, Vec<(f64, f64)>)> = by_model
        .iter()
        .map(|(m, rs)| {
            (
                m.clone(),
                rs.iter()
                    .map(|r| (r.concurrency as f64, r.tokens_per_s))
                    .collect(),
            )
        })
        .collect();
    draw_chart(
        out.join("throughput.html"),
        "Throughput vs concurrency",
        "concurrency (in-flight requests)",
        "output tokens/s",
        &throughput,
        Ideal::LinearScaling,
    )?;

    let speedup: Vec<(String, Vec<(f64, f64)>)> = by_model
        .iter()
        .map(|(m, rs)| {
            // speedup = tokens_per_s / tokens_per_s at this model's lowest level.
            let base = rs
                .iter()
                .min_by_key(|r| r.concurrency)
                .map(|r| r.tokens_per_s)
                .unwrap_or(0.0);
            (
                m.clone(),
                rs.iter()
                    .map(|r| {
                        (
                            r.concurrency as f64,
                            if base > 0.0 {
                                r.tokens_per_s / base
                            } else {
                                0.0
                            },
                        )
                    })
                    .collect(),
            )
        })
        .collect();
    draw_chart(
        out.join("speedup.html"),
        "Parallelization (speedup) vs concurrency",
        "concurrency (in-flight requests)",
        "speedup vs lowest level",
        &speedup,
        Ideal::YEqualsX,
    )?;

    let latency: Vec<(String, Vec<(f64, f64)>)> = by_model
        .iter()
        .map(|(m, rs)| {
            (
                m.clone(),
                rs.iter()
                    .map(|r| (r.concurrency as f64, r.e2e_ms_p95))
                    .collect(),
            )
        })
        .collect();
    draw_chart(
        out.join("latency.html"),
        "Latency vs concurrency",
        "concurrency (in-flight requests)",
        "p95 end-to-end latency (ms)",
        &latency,
        Ideal::Flat,
    )?;

    // TTFT (p95) vs concurrency — this is where the interleaved-prefill straggler tail
    // shows up: continuous batching runs at most one prompt prefill per round, so under
    // bursty closed-loop load a request waits behind preceding admissions and its
    // time-to-first-token climbs. It inflates tail latency, NOT system throughput.
    let ttft: Vec<(String, Vec<(f64, f64)>)> = by_model
        .iter()
        .map(|(m, rs)| {
            (
                m.clone(),
                rs.iter()
                    .map(|r| (r.concurrency as f64, r.ttft_ms_p95))
                    .collect(),
            )
        })
        .collect();
    draw_chart(
        out.join("ttft.html"),
        "Time-to-first-token (p95) vs concurrency",
        "concurrency (in-flight requests)",
        "p95 time-to-first-token (ms)",
        &ttft,
        Ideal::Flat,
    )?;

    // Per-request decode rate (p50) vs concurrency — EXCLUDES the queue + prefill wait,
    // so it stays roughly flat: evidence that decode itself is fast and the tail above is
    // scheduling, not the GPU. Only drawn if the CSV carries the column (>0 somewhere).
    let decode: Vec<(String, Vec<(f64, f64)>)> = by_model
        .iter()
        .map(|(m, rs)| {
            (
                m.clone(),
                rs.iter()
                    .map(|r| (r.concurrency as f64, r.decode_tok_s_p50))
                    .collect(),
            )
        })
        .collect();
    let has_decode = decode.iter().any(|(_, p)| p.iter().any(|(_, y)| *y > 0.0));
    if has_decode {
        draw_chart(
            out.join("decode.html"),
            "Per-request decode rate (p50) vs concurrency",
            "concurrency (in-flight requests)",
            "p50 decode tokens/s (excludes ttft)",
            &decode,
            Ideal::Flat,
        )?;
    }

    write_index(out, &rows, has_decode)?;

    eprintln!(
        ">>> wrote {} (open this), {}, {}, {}, {}{}",
        out.join("index.html").display(),
        out.join("throughput.html").display(),
        out.join("speedup.html").display(),
        out.join("latency.html").display(),
        out.join("ttft.html").display(),
        if has_decode {
            format!(", {}", out.join("decode.html").display())
        } else {
            String::new()
        },
    );
    Ok(())
}

/// Minimal CSV-field escaping: quote a field containing a comma or quote.
fn csv_escape(s: &str) -> String {
    if s.contains(',') || s.contains('"') {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

/// Group rows by model (insertion order), each model's rows sorted by ascending
/// concurrency.
fn group_by_model(rows: &[Row]) -> Vec<(String, Vec<&Row>)> {
    let mut order: Vec<String> = Vec::new();
    for r in rows {
        if !order.contains(&r.model) {
            order.push(r.model.clone());
        }
    }
    order
        .into_iter()
        .map(|m| {
            let mut rs: Vec<&Row> = rows.iter().filter(|r| r.model == m).collect();
            rs.sort_by_key(|r| r.concurrency);
            (m, rs)
        })
        .collect()
}

/// How to draw the theoretical "ideal" reference on a chart, so every chart shows
/// what perfect parallelism would yield next to the measured curve.
#[derive(Clone, Copy)]
enum Ideal {
    /// No reference line.
    None,
    /// A single dashed `y = x` line (perfect linear speedup — the speedup chart).
    YEqualsX,
    /// Per-model dashed line for perfect linear SCALING from the model's
    /// lowest-concurrency point: `ideal(c) = base_y * c / base_c` (throughput).
    LinearScaling,
    /// Per-model dashed FLAT line at the model's lowest-concurrency value:
    /// `ideal(c) = base_y` (decode rate / latency / ttft would be constant under
    /// perfect parallelism with no queueing).
    Flat,
}

/// Render one interactive line chart per the `series` (model -> points) to `path`
/// as a self-contained HTML file via plotly (plotly.js). Each model is one
/// `Scatter` in `LinesMarkers` mode. `ideal` overlays the theoretical reference
/// (see [`Ideal`]), drawn dashed and FIRST so the measured curves sit on top.
fn draw_chart(
    path: std::path::PathBuf,
    title: &str,
    x_label: &str,
    y_label: &str,
    series: &[(String, Vec<(f64, f64)>)],
    ideal: Ideal,
) -> std::io::Result<()> {
    let mut plot = Plot::new();

    // Ideal reference(s), drawn FIRST so the measured curves sit on top.
    match ideal {
        Ideal::None => {}
        Ideal::YEqualsX => {
            let x_max = series
                .iter()
                .flat_map(|(_, p)| p.iter().map(|(x, _)| *x))
                .fold(1.0_f64, f64::max);
            let line = Scatter::new(vec![0.0, x_max], vec![0.0, x_max])
                .name("ideal (linear)")
                .mode(Mode::Lines)
                .line(Line::new().dash(DashType::Dash).color("#888888").width(1.5));
            plot.add_trace(line);
        }
        Ideal::LinearScaling | Ideal::Flat => {
            for (name, pts) in series {
                if pts.is_empty() {
                    continue;
                }
                let (base_c, base_y) = pts
                    .iter()
                    .min_by(|a, b| a.0.partial_cmp(&b.0).unwrap())
                    .copied()
                    .unwrap();
                let xs: Vec<f64> = pts.iter().map(|(x, _)| *x).collect();
                let ys: Vec<f64> = xs
                    .iter()
                    .map(|&x| match ideal {
                        Ideal::LinearScaling if base_c > 0.0 => base_y * x / base_c,
                        _ => base_y,
                    })
                    .collect();
                let line = Scatter::new(xs, ys)
                    .name(format!("{name} · ideal"))
                    .mode(Mode::Lines)
                    .line(Line::new().dash(DashType::Dash).color("#888888").width(1.2));
                plot.add_trace(line);
            }
        }
    }

    // One LinesMarkers scatter per model, named by model so the legend reads cleanly.
    for (name, pts) in series {
        let xs: Vec<f64> = pts.iter().map(|(x, _)| *x).collect();
        let ys: Vec<f64> = pts.iter().map(|(_, y)| *y).collect();
        let trace = Scatter::new(xs, ys)
            .name(name.clone())
            .mode(Mode::LinesMarkers);
        plot.add_trace(trace);
    }

    let layout = Layout::new()
        .title(title)
        .width(900)
        .height(560)
        .x_axis(Axis::new().title(x_label))
        .y_axis(Axis::new().title(y_label));
    plot.set_layout(layout);

    // Self-contained interactive HTML (no kaleido / static-image export).
    plot.write_html(&path);
    Ok(())
}

/// Minimal HTML-text escaping for the index summary table.
fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// Write an `index.html` that gathers every chart in one place: a results table
/// (per model × concurrency, with speedup vs the lowest level) plus linked cards
/// for each chart. Self-contained; open it to navigate the whole sweep.
fn write_index(out: &Path, rows: &[Row], has_decode: bool) -> std::io::Result<()> {
    let by_model = group_by_model(rows);

    let mut table = String::from(
        "<table><thead><tr><th>model</th><th>conc.</th><th>tokens/s</th>\
         <th>speedup</th><th>e2e p95 (s)</th><th>ttft p95 (ms)</th>",
    );
    if has_decode {
        table.push_str("<th>decode p50 (tok/s)</th>");
    }
    table.push_str("</tr></thead><tbody>");
    for (m, rs) in &by_model {
        let base = rs
            .iter()
            .min_by_key(|r| r.concurrency)
            .map(|r| r.tokens_per_s)
            .unwrap_or(0.0);
        for (i, r) in rs.iter().enumerate() {
            let speedup = if base > 0.0 { r.tokens_per_s / base } else { 0.0 };
            let model_cell = if i == 0 {
                format!("<td rowspan=\"{}\"><strong>{}</strong></td>", rs.len(), html_escape(m))
            } else {
                String::new()
            };
            table.push_str(&format!(
                "<tr>{model_cell}<td>{}</td><td>{:.1}</td><td>{:.2}×</td><td>{:.1}</td><td>{:.0}</td>",
                r.concurrency, r.tokens_per_s, speedup, r.e2e_ms_p95 / 1000.0, r.ttft_ms_p95
            ));
            if has_decode {
                table.push_str(&format!("<td>{:.1}</td>", r.decode_tok_s_p50));
            }
            table.push_str("</tr>");
        }
    }
    table.push_str("</tbody></table>");

    let mut charts: Vec<(&str, &str, &str)> = vec![
        ("throughput.html", "Throughput vs concurrency",
         "Aggregate system tokens/s as concurrency rises — the headline scaling curve, with each model's dashed ideal (perfect linear scaling from its lowest level)."),
        ("speedup.html", "Parallelization (speedup)",
         "Speedup vs the lowest concurrency level, against the dashed ideal (linear) y=x."),
        ("latency.html", "Latency (p95)",
         "p95 end-to-end latency vs concurrency; the dashed flat line is the ideal no-queueing floor (the lowest-level latency)."),
        ("ttft.html", "Time-to-first-token (p95)",
         "p95 TTFT — where the interleaved-prefill straggler tail shows up; dashed flat = ideal floor."),
    ];
    if has_decode {
        charts.push(("decode.html", "Per-request decode rate (p50)",
            "Steady-state decode tok/s (excludes ttft); dashed flat = ideal constant — evidence the tail is scheduling, not the GPU."));
    }
    let cards: String = charts
        .iter()
        .map(|(file, title, desc)| {
            format!(
                "<a class=\"card\" href=\"{file}\"><div class=\"ct\">{title}</div><div class=\"cd\">{desc}</div></a>"
            )
        })
        .collect();

    let html = format!(
        "<!DOCTYPE html><html lang=\"en\"><head><meta charset=\"utf-8\">\
<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
<title>burn-lm-bench results</title><style>\
:root{{--ink:#202326;--muted:#687076;--line:#d9d2c4;--paper:#f7f4ee;--blue:#285f9f;--cyan:#00818a;}}\
*{{box-sizing:border-box}}body{{margin:0;background:var(--paper);color:var(--ink);font:16px/1.6 ui-sans-serif,system-ui,sans-serif}}\
main{{max-width:1000px;margin:0 auto;padding:40px 24px 80px}}\
h1{{font-size:30px;margin:0 0 4px}}.dek{{color:#42484d;margin:0 0 24px}}\
h2{{font-size:19px;border-top:2px solid var(--ink);padding-top:10px;margin:32px 0 12px}}\
table{{width:100%;border-collapse:collapse;font-size:13.5px;background:#fffdf8}}\
th,td{{padding:7px 10px;border:1px solid var(--line);text-align:right}}th{{background:#e8dfcf;font-weight:800}}\
td:first-child,th:first-child,td:nth-child(2){{text-align:left}}\
.cards{{display:grid;grid-template-columns:repeat(auto-fill,minmax(280px,1fr));gap:14px}}\
.card{{display:block;border:1px solid var(--line);border-radius:10px;padding:14px 16px;background:#fffdf8;text-decoration:none;color:inherit}}\
.card:hover{{background:#ece4d5}}.ct{{font-weight:800;color:var(--blue)}}.cd{{font-size:13px;color:#42484d;margin-top:4px}}\
.eyebrow{{font-size:12px;letter-spacing:.16em;text-transform:uppercase;color:var(--cyan);font-weight:800}}\
</style></head><body><main>\
<div class=\"eyebrow\">burn-lm continuous batching · PR #57</div>\
<h1>Bench results</h1>\
<p class=\"dek\">Model × concurrency sweep. <strong>Speedup</strong> is vs each model's lowest level; \
every chart overlays the dashed <strong>ideal</strong> (perfect parallelism).</p>\
<h2>Summary</h2>{table}\
<h2>Charts</h2><div class=\"cards\">{cards}</div>\
</main></body></html>"
    );
    std::fs::write(out.join("index.html"), html)
}
