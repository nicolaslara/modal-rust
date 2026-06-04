//! CBOR codec for Modal function arguments and results.
//!
//! Modal's non-Python SDKs encode the call payload as CBOR of the 2-tuple
//! `(args, kwargs)` — `args` a positional sequence, `kwargs` a map. We advertise
//! and request `DATA_FORMAT_CBOR` end-to-end (spec §6) so invoke/result stays in
//! CBOR; `PICKLE` is treated as an opaque passthrough until a later milestone.
//!
//! These are thin `serde_cbor` wrappers (mirrors modal-rs `cbor.rs`) with
//! codec-specific error mapping.

use serde::{de::DeserializeOwned, Serialize};

use crate::error::{Error, Result};

/// Encode a value to CBOR bytes. For function calls, `value` is the
/// `(args, kwargs)` tuple.
pub fn encode<T: Serialize>(value: &T) -> Result<Vec<u8>> {
    serde_cbor::to_vec(value).map_err(|e| Error::codec(format!("cbor encode: {e}")))
}

/// Decode a value from CBOR bytes (e.g. a `GenericResult` data payload).
pub fn decode<T: DeserializeOwned>(bytes: &[u8]) -> Result<T> {
    serde_cbor::from_slice(bytes).map_err(|e| Error::codec(format!("cbor decode: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn round_trip_args_kwargs_tuple() {
        // Mirror the spike's invoke shape: ((payload,), {}).
        let args = vec![1_i64, 2, 3];
        let kwargs: HashMap<String, i64> = HashMap::new();
        let payload = (args.clone(), kwargs);

        let encoded = encode(&payload).unwrap();
        let (decoded_args, decoded_kwargs): (Vec<i64>, HashMap<String, i64>) =
            decode(&encoded).unwrap();

        assert_eq!(decoded_args, args);
        assert!(decoded_kwargs.is_empty());
    }

    #[test]
    fn decode_rejects_garbage() {
        let result: Result<(Vec<i64>, HashMap<String, i64>)> = decode(&[0xff, 0x00, 0x13, 0x37]);
        assert!(result.is_err());
    }
}
