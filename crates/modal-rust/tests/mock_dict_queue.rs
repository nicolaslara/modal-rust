//! OFFLINE Dict/Queue round-trips: the FACADE handles (`modal_rust::Dict` /
//! `modal_rust::Queue`) against the in-process mock backend's STATEFUL store
//! (`modal-rust-testkit`), end-to-end through the real gRPC transport on
//! loopback — no Modal credentials, no network, no Python (except the one
//! python3-gated interop test, which skips cleanly when python3 is absent).
//!
//! What this proves beyond the SDK unit tests: the typed facade surface
//! (pickle codec + ops + poll loop) does GENUINE state transitions — put→get
//! round-trips, FIFO order, put_if_absent semantics, blocking-get unblocked by
//! a concurrent producer — and puts the PINNED key bytes on the wire.

use std::io::Write;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use modal_rust::{Dict, Queue};
use modal_rust_testkit::prelude::*;
use serde::{Deserialize, Serialize};

/// A struct value (maps to a Python dict under the restricted-pickle codec).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Job {
    id: i64,
    name: String,
}

// ---------------------------------------------------------------- Dict ----

/// The core Dict surface, typed, against real mock state:
/// put / get / contains / len / pop / clear (+ absent-key behavior).
#[tokio::test]
async fn dict_typed_round_trip() {
    let mock = MockModal::start().await.expect("mock up");
    let d = Dict::from_name_at("scores", mock.url())
        .await
        .expect("resolve");
    assert_eq!(d.dict_id(), "di-1");
    assert_eq!(d.name(), "scores");

    // Heterogeneous values, like Python (per-call generics).
    d.put("alice", &42_i64).await.expect("put int");
    d.put(
        "job",
        &Job {
            id: 7,
            name: "resize".into(),
        },
    )
    .await
    .expect("put struct");

    assert_eq!(d.get::<i64>("alice").await.expect("get"), Some(42));
    assert_eq!(
        d.get::<Job>("job").await.expect("get struct"),
        Some(Job {
            id: 7,
            name: "resize".into()
        })
    );
    assert_eq!(d.get::<i64>("absent").await.expect("get miss"), None);

    assert!(d.contains("alice").await.expect("contains hit"));
    assert!(!d.contains("absent").await.expect("contains miss"));
    assert_eq!(d.len().await.expect("len"), 2);

    // pop removes + returns; a second pop is None.
    assert_eq!(d.pop::<i64>("alice").await.expect("pop"), Some(42));
    assert_eq!(d.pop::<i64>("alice").await.expect("pop again"), None);
    assert_eq!(d.len().await.expect("len after pop"), 1);

    d.clear().await.expect("clear");
    assert_eq!(d.len().await.expect("len after clear"), 0);
    assert_eq!(d.get::<Job>("job").await.expect("get after clear"), None);
}

/// `put` overwrites; `put_if_absent` inserts only when missing and reports it.
#[tokio::test]
async fn dict_put_if_absent_semantics() {
    let mock = MockModal::start().await.expect("mock up");
    let d = Dict::from_name_at("flags", mock.url())
        .await
        .expect("resolve");

    assert!(d.put_if_absent("k", &1_i64).await.expect("first insert"));
    assert!(!d
        .put_if_absent("k", &2_i64)
        .await
        .expect("second is a no-op"));
    // The stored value is UNCHANGED by the losing put_if_absent…
    assert_eq!(d.get::<i64>("k").await.expect("get"), Some(1));
    // …while a plain put overwrites.
    d.put("k", &3_i64).await.expect("overwrite");
    assert_eq!(d.get::<i64>("k").await.expect("get"), Some(3));

    // The wire carries the if_not_exists flag (DictUpdate is the only put RPC).
    let updates = mock.requests::<DictUpdateRequest>();
    assert_eq!(updates.len(), 3);
    assert!(updates[0].if_not_exists && updates[1].if_not_exists);
    assert!(!updates[2].if_not_exists);
}

/// The PINNED key bytes ("foo" as byte-exact CPython protocol-4 pickle — the
/// Python key-lookup contract) ride the wire from the typed facade methods,
/// and the raw escape hatch sees those exact bytes.
#[tokio::test]
async fn dict_key_bytes_on_the_wire_are_pinned_pickle() {
    const FOO_KEY: &[u8] = &[
        0x80, 0x04, 0x95, 0x07, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x8c, 0x03, 0x66, 0x6f,
        0x6f, 0x94, 0x2e,
    ];
    let mock = MockModal::start().await.expect("mock up");
    let d = Dict::from_name_at("pins", mock.url())
        .await
        .expect("resolve");

    d.put("foo", &1_i64).await.expect("put");
    let update = mock
        .last::<DictUpdateRequest>()
        .expect("DictUpdate recorded");
    assert_eq!(update.updates.len(), 1);
    assert_eq!(
        update.updates[0].key, FOO_KEY,
        "key bytes must be the pinned pickle"
    );

    // The raw escape hatch interoperates with the typed surface byte-for-byte.
    assert!(d.contains_raw(FOO_KEY).await.expect("raw contains"));
    let raw = d.get_raw(FOO_KEY).await.expect("raw get").expect("present");
    assert_eq!(d.get::<i64>("foo").await.expect("typed get"), Some(1));
    assert_eq!(
        raw,
        modal_rust::sdk::pickle::encode_value(&1_i64).expect("encode")
    );
}

/// CREATE_IF_MISSING is idempotent: two handles resolved from the SAME name
/// (separate connections) get the SAME id and observe shared state.
#[tokio::test]
async fn dict_same_name_resolves_to_shared_state() {
    let mock = MockModal::start().await.expect("mock up");
    let a = Dict::from_name_at("shared", mock.url())
        .await
        .expect("resolve a");
    let b = Dict::from_name_at("shared", mock.url())
        .await
        .expect("resolve b");
    assert_eq!(a.dict_id(), b.dict_id());

    a.put("k", &"hello".to_string()).await.expect("put via a");
    assert_eq!(
        b.get::<String>("k").await.expect("get via b"),
        Some("hello".to_string())
    );
}

/// A PRESENT key whose value does not decode into the requested type is a
/// typed codec Err through the full stack — never a silent None.
#[tokio::test]
async fn dict_wrong_type_get_is_codec_error() {
    let mock = MockModal::start().await.expect("mock up");
    let d = Dict::from_name_at("types", mock.url())
        .await
        .expect("resolve");
    d.put("k", &"a string").await.expect("put");
    let got = d.get::<i64>("k").await;
    assert!(got.is_err(), "wrong-type get must be Err, got {got:?}");
}

// --------------------------------------------------------------- Queue ----

/// The core Queue surface: put / put_many / len / get_many / FIFO order /
/// clear, with non-blocking polls (Some(ZERO)).
#[tokio::test]
async fn queue_typed_round_trip_fifo() {
    let mock = MockModal::start().await.expect("mock up");
    let q = Queue::from_name_at("jobs", mock.url())
        .await
        .expect("resolve");
    assert_eq!(q.queue_id(), "qu-1");

    q.put(&Job {
        id: 1,
        name: "a".into(),
    })
    .await
    .expect("put");
    q.put_many(&[
        Job {
            id: 2,
            name: "b".into(),
        },
        Job {
            id: 3,
            name: "c".into(),
        },
    ])
    .await
    .expect("put_many");
    assert_eq!(q.len().await.expect("len"), 3);

    // put_many is ONE QueuePut RPC with repeated values (no QueuePutMany).
    let puts = mock.requests::<QueuePutRequest>();
    assert_eq!(puts.len(), 2);
    assert_eq!(puts[1].values.len(), 2);

    // FIFO: first single get, then a batch drains the rest in order.
    let first: Option<Job> = q.get(Some(Duration::ZERO)).await.expect("get");
    assert_eq!(
        first,
        Some(Job {
            id: 1,
            name: "a".into()
        })
    );
    let rest: Vec<Job> = q
        .get_many(10, Some(Duration::ZERO))
        .await
        .expect("get_many");
    assert_eq!(
        rest,
        vec![
            Job {
                id: 2,
                name: "b".into()
            },
            Job {
                id: 3,
                name: "c".into()
            }
        ]
    );
    assert_eq!(q.len().await.expect("len drained"), 0);

    q.put_many(&[1_i64, 2]).await.expect("refill");
    q.clear().await.expect("clear");
    assert_eq!(q.len().await.expect("len after clear"), 0);
}

/// `Some(d)` on an empty queue waits ~d then returns Ok(None) / Ok(vec![]) —
/// the timeout is honored (server-side blocking window in the mock) and the
/// empty result is NOT an error.
#[tokio::test]
async fn queue_get_timeout_returns_none() {
    let mock = MockModal::start().await.expect("mock up");
    let q = Queue::from_name_at("empty", mock.url())
        .await
        .expect("resolve");

    let start = Instant::now();
    let got: Option<i64> = q.get(Some(Duration::from_millis(150))).await.expect("get");
    let waited = start.elapsed();
    assert_eq!(got, None);
    assert!(
        waited >= Duration::from_millis(100),
        "returned too early: {waited:?}"
    );

    let batch: Vec<i64> = q
        .get_many(5, Some(Duration::from_millis(50)))
        .await
        .expect("get_many");
    assert!(batch.is_empty());
}

/// `get(None)` blocks until an item arrives: a SECOND handle on the same name
/// (separate connection — the same-handle client lock is documented to
/// serialize clones) puts mid-wait and unblocks the consumer.
#[tokio::test]
async fn queue_blocking_get_unblocked_by_concurrent_put() {
    let mock = MockModal::start().await.expect("mock up");
    let consumer = Queue::from_name_at("pipe", mock.url())
        .await
        .expect("resolve consumer");
    let producer = Queue::from_name_at("pipe", mock.url())
        .await
        .expect("resolve producer");
    assert_eq!(consumer.queue_id(), producer.queue_id());

    let feeder = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        producer.put(&99_i64).await.expect("producer put");
    });

    // Blocks (no timeout) until the producer's item lands.
    let got: Option<i64> = consumer.get(None).await.expect("blocking get");
    assert_eq!(got, Some(99));
    feeder.await.expect("feeder task");
}

/// First-item-blocks batching: `get_many(n, ..)` returns whatever is available
/// once ANYTHING is (≤ n), not waiting to fill the batch.
#[tokio::test]
async fn queue_get_many_caps_at_n() {
    let mock = MockModal::start().await.expect("mock up");
    let q = Queue::from_name_at("batch", mock.url())
        .await
        .expect("resolve");
    q.put_many(&[1_i64, 2, 3, 4, 5]).await.expect("put_many");

    let two: Vec<i64> = q.get_many(2, Some(Duration::ZERO)).await.expect("get 2");
    assert_eq!(two, vec![1, 2]);
    // Asking for more than remains returns what's there.
    let rest: Vec<i64> = q
        .get_many(10, Some(Duration::ZERO))
        .await
        .expect("get rest");
    assert_eq!(rest, vec![3, 4, 5]);
}

// ------------------------------------------- python3 interop (gated) ----

/// END-TO-END Python interop through the facade + mock: python3 pickles a
/// value, it rides `put_raw` into the store, and the TYPED `get`/`get` decode
/// it; a Rust-written value's stored bytes unpickle in python3. Skips cleanly
/// when python3 is absent (the codec-level pinned-bytes + interop tests live
/// in `modal_rust_sdk::pickle`).
#[tokio::test]
async fn python3_interop_through_facade() {
    let probe = Command::new("python3").arg("--version").output();
    if !probe.map(|o| o.status.success()).unwrap_or(false) {
        eprintln!("python3 not found; skipping facade interop test");
        return;
    }

    let mock = MockModal::start().await.expect("mock up");
    let d = Dict::from_name_at("interop", mock.url())
        .await
        .expect("resolve");

    // Python -> Rust: store python3's protocol-4 bytes under the PINNED "foo"
    // key bytes (what a Python writer would send), read typed via the facade.
    let py_value = Command::new("python3")
        .args(["-c", "import pickle,sys; sys.stdout.buffer.write(pickle.dumps({'id': 7, 'name': 'resize'}, protocol=4))"])
        .output()
        .expect("python3 dumps");
    assert!(py_value.status.success());
    let key = modal_rust::sdk::pickle::encode_str_key("foo");
    d.put_raw(&key, &py_value.stdout)
        .await
        .expect("put_raw python bytes");
    assert_eq!(
        d.get::<Job>("foo")
            .await
            .expect("typed get of python value"),
        Some(Job {
            id: 7,
            name: "resize".into()
        })
    );

    // Rust -> Python: the bytes the facade stored for a typed put unpickle in
    // python3 to the expected plain dict.
    d.put(
        "job",
        &Job {
            id: 9,
            name: "encode".into(),
        },
    )
    .await
    .expect("put");
    let stored = d
        .get_raw(&modal_rust::sdk::pickle::encode_str_key("job"))
        .await
        .expect("get_raw")
        .expect("present");
    let mut child = Command::new("python3")
        .args([
            "-c",
            "import pickle,sys; v=pickle.loads(sys.stdin.buffer.read()); assert v=={'id':9,'name':'encode'}, v",
        ])
        .stdin(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("spawn python3");
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(&stored)
        .expect("write");
    let status = child.wait().expect("python3 exit");
    assert!(
        status.success(),
        "python3 failed to load the Rust-written value"
    );
}
