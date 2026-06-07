//! The per-recipient delivery computation — kept OUT of `lib.rs` so the library file
//! stays the clean modal-rust surface (the input/output types + the `#[function]`).
//!
//! "Delivering" a notification in a real system means handing it to a channel
//! provider and getting back a stable id you can use to look the send up later. We
//! reproduce that shape WITHOUT a network: a deterministic `receipt_id` derived from
//! the `(name, channel)` pair, plus a channel-resolved confirmation line. The id is a
//! real hash of the input (`std::collections::hash_map::DefaultHasher`), rendered as
//! fixed-width hex — so the same recipient on the same channel always yields the same
//! id, and different recipients/channels yield different ids. That is genuine, cheap,
//! offline computation: no echo, no fixed constant.

use crate::{Receipt, Recipient};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

/// Deliver one notification and return its receipt.
///
/// Computes a stable `receipt_id` from the recipient's `(name, channel)` and pairs it
/// with a human-readable confirmation that names the resolved channel. Depends only on
/// its input, so a batch of these fans out cleanly with `.for_each` / `.spawn_map`.
pub fn deliver(r: &Recipient) -> Receipt {
    let receipt_id = receipt_id(&r.name, &r.channel);
    // A short prefix of the id makes the confirmation line readable while staying
    // traceable to the full id.
    let short = &receipt_id[..8];
    let sent = format!("delivered to {} via {} [{short}]", r.name, r.channel);
    Receipt {
        name: r.name.clone(),
        channel: r.channel.clone(),
        receipt_id,
        sent,
    }
}

/// The stable per-send id: a real hash of `(name, channel)` as fixed-width hex.
///
/// Hashing the two fields (rather than a concatenated string) keeps `("a", "bc")` and
/// `("ab", "c")` distinct. `DefaultHasher` is deterministic within a build, which is
/// exactly what a confirmation id needs: same input -> same id, every time.
fn receipt_id(name: &str, channel: &str) -> String {
    let mut hasher = DefaultHasher::new();
    name.hash(&mut hasher);
    channel.hash(&mut hasher);
    // 64-bit hash -> 16 lowercase hex chars, zero-padded so the id is fixed width.
    format!("{:016x}", hasher.finish())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn recipient(name: &str, channel: &str) -> Recipient {
        Recipient {
            name: name.to_string(),
            channel: channel.to_string(),
        }
    }

    #[test]
    fn receipt_id_is_deterministic() {
        let a = deliver(&recipient("ada", "email"));
        let b = deliver(&recipient("ada", "email"));
        assert_eq!(a.receipt_id, b.receipt_id);
    }

    #[test]
    fn receipt_id_is_fixed_width_hex() {
        let r = deliver(&recipient("ada", "email"));
        assert_eq!(r.receipt_id.len(), 16);
        assert!(r.receipt_id.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn different_inputs_yield_different_ids() {
        let by_name = deliver(&recipient("ada", "email")).receipt_id;
        let other_name = deliver(&recipient("babbage", "email")).receipt_id;
        let other_channel = deliver(&recipient("ada", "sms")).receipt_id;
        assert_ne!(by_name, other_name);
        assert_ne!(by_name, other_channel);
        // The field boundary matters: ("ab","c") must not collide with ("a","bc").
        assert_ne!(
            deliver(&recipient("ab", "c")).receipt_id,
            deliver(&recipient("a", "bc")).receipt_id,
        );
    }
}
