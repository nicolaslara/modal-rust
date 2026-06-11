//! Pickle codec for Modal Dict/Queue payloads (the Python interop boundary).
//!
//! Dict/Queue messages carry opaque `bytes` with **no wire-level format field**
//! (contrast function IO's `DataFormat` + [`crate::codec`] CBOR). Encoding is a
//! pure client-side convention, and Modal's Python client pickles everything
//! (`modal._serialization.serialize()`, cloudpickle at `PICKLE_PROTOCOL = 4`).
//! Modal's own non-Python clients follow suit (Go via og-rek, JS via a
//! hand-written `pickle.ts`), so this module mirrors that precedent with a
//! restricted pickle codec (design: `docs/local/dict-queue-design.md` §1):
//!
//! - **Dict keys** ([`encode_str_key`]): `&str` only, emitted as **byte-exact
//!   CPython protocol-4 pickle**. The server matches Dict keys by byte-equality
//!   on the serialized key, so only byte-identical pickle gives bidirectional
//!   key lookup with Python.
//! - **Values** ([`encode_value`] / [`decode_value`]): any
//!   `Serialize`/`DeserializeOwned` via `serde-pickle` — encodes protocol 3
//!   (readable by any Python 3), decodes protocols 0–5, which covers
//!   Python-written *plain data* (cloudpickle output for primitives / lists /
//!   dicts / bytes is standard pickle opcodes). Rust structs map to Python
//!   dicts. A Python-pickled **custom class / function** fails decode with a
//!   typed [`Error::Codec`] naming the boundary — plain data interops;
//!   arbitrary Python objects do not, by design. Raw-byte escape hatches
//!   (`get_raw` / `put_raw` on the facade handles) bypass this codec entirely.

use serde::{de::DeserializeOwned, Serialize};

use crate::error::{Error, Result};

// CPython pickle opcodes used by the protocol-4 string-key encoder.
const PROTO: u8 = 0x80;
const FRAME: u8 = 0x95;
const SHORT_BINUNICODE: u8 = 0x8c; // utf-8 length <= 0xff (1-byte length)
const BINUNICODE: u8 = 0x58; // 'X': 4-byte little-endian length
const BINUNICODE8: u8 = 0x8d; // 8-byte little-endian length (proto 4)
const MEMOIZE: u8 = 0x94;
const STOP: u8 = 0x2e;

/// Encode a Dict key as **byte-exact CPython protocol-4 pickle** of the string.
///
/// Matches `pickle.dumps(key, protocol=4)` byte-for-byte: `PROTO 4`, one
/// `FRAME` covering the body, `SHORT_BINUNICODE`/`BINUNICODE`/`BINUNICODE8`
/// (chosen by utf-8 byte length, exactly like CPython's `save_str`),
/// `MEMOIZE`, `STOP`. Deterministic for `str`, so Rust-written keys are
/// loadable from Python and vice versa (the server compares raw key bytes).
pub fn encode_str_key(key: &str) -> Vec<u8> {
    let utf8 = key.as_bytes();

    // The string opcode + its length prefix, CPython save_str's exact branch
    // order for protocol 4.
    let mut body: Vec<u8> = Vec::with_capacity(utf8.len() + 11);
    if utf8.len() <= 0xff {
        body.push(SHORT_BINUNICODE);
        body.push(utf8.len() as u8);
    } else if u64::try_from(utf8.len()).unwrap_or(u64::MAX) > 0xffff_ffff {
        body.push(BINUNICODE8);
        body.extend_from_slice(&(utf8.len() as u64).to_le_bytes());
    } else {
        body.push(BINUNICODE);
        body.extend_from_slice(&(utf8.len() as u32).to_le_bytes());
    }
    body.extend_from_slice(utf8);
    body.push(MEMOIZE);
    body.push(STOP);

    // PROTO 4 header + a single FRAME spanning the whole body (CPython's framer
    // always commits one frame at end_framing, even for tiny pickles).
    let mut out: Vec<u8> = Vec::with_capacity(body.len() + 11);
    out.push(PROTO);
    out.push(4);
    out.push(FRAME);
    out.extend_from_slice(&(body.len() as u64).to_le_bytes());
    out.extend_from_slice(&body);
    out
}

/// Encode a value to Python-readable pickle bytes (protocol 3 via
/// `serde-pickle`). Rust structs become Python dicts (the same plain-object
/// convention as Modal's JS client).
pub fn encode_value<T: Serialize>(value: &T) -> Result<Vec<u8>> {
    serde_pickle::to_vec(value, serde_pickle::SerOptions::new())
        .map_err(|e| Error::codec(format!("pickle encode: {e}")))
}

/// Decode pickle bytes (protocols 0–5) into a Rust value.
///
/// Covers Python-written plain data (str/int/float/bool/bytes/lists/dicts —
/// including cloudpickle's protocol-4 FRAME/MEMOIZE output). A payload that
/// pickles an arbitrary **Python object** (class instance, function, custom
/// type) fails with a typed [`Error::Codec`] explaining the interop boundary —
/// never a panic, never a silent `None`.
pub fn decode_value<T: DeserializeOwned>(bytes: &[u8]) -> Result<T> {
    serde_pickle::from_slice(bytes, serde_pickle::DeOptions::new()).map_err(map_decode_error)
}

/// Map `serde-pickle` decode failures into [`Error::Codec`], distinguishing the
/// honest interop boundary (a pickled Python object we refuse by design) from
/// plain corruption / type mismatches.
fn map_decode_error(e: serde_pickle::Error) -> Error {
    use serde_pickle::{Error as PErr, ErrorCode};
    let is_python_object = |code: &ErrorCode| {
        matches!(
            code,
            ErrorCode::Unsupported(_)
                | ErrorCode::UnresolvedGlobal
                | ErrorCode::UnsupportedGlobal(_, _)
        )
    };
    match &e {
        PErr::Eval(code, _) | PErr::Syntax(code) if is_python_object(code) => {
            Error::codec(format!(
                "pickle decode: payload contains a Python object (class/function/custom type) \
                 that cannot be decoded into Rust plain data — only \
                 str/int/float/bool/bytes/lists/dicts interop; use the raw-bytes methods \
                 (get_raw/put_raw) to pass it through opaquely ({e})"
            ))
        }
        _ => Error::codec(format!("pickle decode: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;
    use std::collections::BTreeMap;
    use std::io::Write as _;
    use std::process::{Command, Stdio};

    // ---- encode_str_key: pinned bytes against CPython `pickle.dumps(s, protocol=4)` ----

    #[test]
    fn key_foo_is_byte_exact_cpython_protocol_4() {
        // python3: pickle.dumps("foo", 4).hex()
        assert_eq!(
            encode_str_key("foo"),
            [
                0x80, 0x04, 0x95, 0x07, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x8c, 0x03, 0x66,
                0x6f, 0x6f, 0x94, 0x2e
            ]
        );
    }

    #[test]
    fn key_empty_string() {
        // python3: pickle.dumps("", 4).hex()
        assert_eq!(
            encode_str_key(""),
            [
                0x80, 0x04, 0x95, 0x04, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x8c, 0x00, 0x94,
                0x2e
            ]
        );
    }

    #[test]
    fn key_non_ascii_uses_utf8_byte_length() {
        // python3: pickle.dumps("héllo→日本", 4).hex() — 8 chars, 15 utf-8 bytes.
        assert_eq!(
            encode_str_key("héllo→日本"),
            [
                0x80, 0x04, 0x95, 0x13, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x8c, 0x0f, 0x68,
                0xc3, 0xa9, 0x6c, 0x6c, 0x6f, 0xe2, 0x86, 0x92, 0xe6, 0x97, 0xa5, 0xe6, 0x9c, 0xac,
                0x94, 0x2e
            ]
        );
    }

    #[test]
    fn key_255_bytes_stays_short_binunicode() {
        // python3: pickle.dumps("b"*255, 4) — frame len 0x0103, SHORT_BINUNICODE 0xff.
        let got = encode_str_key(&"b".repeat(255));
        assert_eq!(got.len(), 270);
        assert_eq!(
            &got[..14],
            [0x80, 0x04, 0x95, 0x03, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x8c, 0xff, 0x62]
        );
        assert_eq!(&got[268..], [0x94, 0x2e]);
    }

    #[test]
    fn key_over_255_bytes_uses_binunicode() {
        // python3: pickle.dumps("a"*300, 4) — BINUNICODE ('X') + u32 LE length.
        let got = encode_str_key(&"a".repeat(300));
        assert_eq!(got.len(), 318);
        assert_eq!(
            &got[..17],
            [
                0x80, 0x04, 0x95, 0x33, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x58, 0x2c, 0x01,
                0x00, 0x00, 0x61
            ]
        );
        assert!(got[16..316].iter().all(|&b| b == 0x61));
        assert_eq!(&got[316..], [0x94, 0x2e]);

        // The 256-byte boundary: python3 pickle.dumps("b"*256, 4) prefix.
        let got = encode_str_key(&"b".repeat(256));
        assert_eq!(got.len(), 274);
        assert_eq!(
            &got[..17],
            [
                0x80, 0x04, 0x95, 0x07, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x58, 0x00, 0x01,
                0x00, 0x00, 0x62
            ]
        );
    }

    // ---- value codec round-trips ----

    #[test]
    fn value_round_trip_i64() {
        let encoded = encode_value(&42_i64).unwrap();
        // serde-pickle default = protocol 3 header (\x80\x03): Python-3 readable.
        assert_eq!(&encoded[..2], [0x80, 0x03]);
        let decoded: i64 = decode_value(&encoded).unwrap();
        assert_eq!(decoded, 42);
    }

    #[test]
    fn value_round_trip_string() {
        let encoded = encode_value(&"hello pickle".to_string()).unwrap();
        let decoded: String = decode_value(&encoded).unwrap();
        assert_eq!(decoded, "hello pickle");
    }

    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    struct Job {
        id: u32,
        name: String,
        weights: Vec<f64>,
    }

    #[test]
    fn value_round_trip_struct_as_dict() {
        let job = Job {
            id: 7,
            name: "resize".into(),
            weights: vec![0.5, 1.25],
        };
        let encoded = encode_value(&job).unwrap();
        let decoded: Job = decode_value(&encoded).unwrap();
        assert_eq!(decoded, job);
        // Structs are Python dicts on the wire (JS plain-object convention):
        // the same bytes decode as a map too.
        let as_map: BTreeMap<String, serde_pickle::Value> = decode_value(&encoded).unwrap();
        assert!(as_map.contains_key("id") && as_map.contains_key("weights"));
    }

    #[test]
    fn value_round_trip_vec() {
        let v = vec![1_i64, 2, 3, -9];
        let encoded = encode_value(&v).unwrap();
        let decoded: Vec<i64> = decode_value(&encoded).unwrap();
        assert_eq!(decoded, v);
    }

    #[test]
    fn wrong_type_decode_is_codec_error_not_panic() {
        let encoded = encode_value(&"definitely not an int").unwrap();
        let res: Result<i64> = decode_value(&encoded);
        assert!(matches!(res, Err(Error::Codec(_))), "got: {res:?}");
    }

    // ---- the honest boundary: pickled Python objects ----

    #[test]
    fn python_object_payload_fails_with_typed_error() {
        // python3: pickle.dumps(len, 4) — a builtins.len global reference
        // (STACK_GLOBAL), the canonical "arbitrary Python object" payload.
        let py_object: &[u8] = &[
            0x80, 0x04, 0x95, 0x14, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x8c, 0x08, 0x62,
            0x75, 0x69, 0x6c, 0x74, 0x69, 0x6e, 0x73, 0x94, 0x8c, 0x03, 0x6c, 0x65, 0x6e, 0x94,
            0x93, 0x94, 0x2e,
        ];
        let res: Result<serde_pickle::Value> = decode_value(py_object);
        match res {
            Err(Error::Codec(msg)) => {
                assert!(
                    msg.contains("Python object"),
                    "error must name the interop boundary, got: {msg}"
                );
            }
            other => panic!("expected typed Codec error, got: {other:?}"),
        }
    }

    #[test]
    fn garbage_decode_is_codec_error() {
        let res: Result<i64> = decode_value(&[0xff, 0x00, 0x13, 0x37]);
        assert!(matches!(res, Err(Error::Codec(_))));
    }

    // ---- live python3 interop (skips cleanly where python3 is absent) ----

    /// Round-trip through a real CPython: our key + value bytes load in Python,
    /// and Python's own protocol-4 dumps (FRAME/MEMOIZE) decode in Rust.
    #[test]
    fn python3_interop_round_trip() {
        let probe = Command::new("python3").arg("--version").output();
        if !probe.map(|o| o.status.success()).unwrap_or(false) {
            eprintln!("python3 not found; skipping interop test");
            return;
        }

        // Rust -> Python: key bytes + a struct value must unpickle to ("foo", dict).
        let key = encode_str_key("foo");
        let value = encode_value(&Job {
            id: 7,
            name: "resize".into(),
            weights: vec![0.5, 1.25],
        })
        .unwrap();
        let script = r#"
import pickle, sys
data = sys.stdin.buffer.read()
key_len = int.from_bytes(data[:4], "little")
key = pickle.loads(data[4 : 4 + key_len])
value = pickle.loads(data[4 + key_len :])
assert key == "foo", key
assert value == {"id": 7, "name": "resize", "weights": [0.5, 1.25]}, value
# Python -> Rust: emit plain data at protocol 4 (FRAME + MEMOIZE, like cloudpickle).
sys.stdout.buffer.write(pickle.dumps({"answer": 42, "tags": ["a", "b"]}, protocol=4))
"#;
        let mut child = Command::new("python3")
            .args(["-c", script])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("spawn python3");
        {
            let stdin = child.stdin.as_mut().unwrap();
            stdin
                .write_all(&(key.len() as u32).to_le_bytes())
                .and_then(|()| stdin.write_all(&key))
                .and_then(|()| stdin.write_all(&value))
                .expect("write to python3");
        }
        let out = child.wait_with_output().expect("python3 exit");
        assert!(out.status.success(), "python3 assertions failed");

        #[derive(Debug, PartialEq, Deserialize)]
        struct Plain {
            answer: i64,
            tags: Vec<String>,
        }
        let decoded: Plain = decode_value(&out.stdout).expect("decode python3 protocol-4 output");
        assert_eq!(
            decoded,
            Plain {
                answer: 42,
                tags: vec!["a".into(), "b".into()]
            }
        );
    }
}
