//! The real work a single scheduled run performs: a roll-up (group-by aggregation)
//! over the run's input events. This is the example's COMPUTE, kept off the
//! modal-rust surface in `lib.rs` so that file stays the clean
//! types-plus-`#[function]` layer. It is plain Rust — no Modal, no scheduling — and
//! deterministic: the same events always produce the same [`Rollup`].

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// One observation the run aggregates — `count` occurrences attributed to `source`.
/// A real scheduled job would read these from a table or a log; here they ride in on
/// the [`crate::Tick`] so the roll-up runs deterministically offline.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Event {
    /// Which bucket the count belongs to (e.g. a service name, a region, a user).
    pub source: String,
    /// How many occurrences this event records.
    pub count: u64,
}

/// The aggregate a roll-up produces over a batch of [`Event`]s.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rollup {
    /// Total occurrences across every event — the sum of all `count`s.
    pub total: u64,
    /// Per-source totals, grouped by `source`. A `BTreeMap` keeps the keys ordered so
    /// the roll-up is deterministic regardless of input order.
    pub by_source: BTreeMap<String, u64>,
    /// The source with the highest total, or `None` when there were no events. Ties
    /// break deterministically toward the lexicographically smallest source.
    pub busiest: Option<String>,
}

/// Roll up `events` into a [`Rollup`]: sum every count, group the counts by source,
/// and pick the busiest source. Deterministic — input order does not affect the
/// result, and ties on the busiest source resolve to the smallest source name.
pub fn roll_up(events: &[Event]) -> Rollup {
    let mut by_source: BTreeMap<String, u64> = BTreeMap::new();
    let mut total: u64 = 0;
    for event in events {
        total += event.count;
        *by_source.entry(event.source.clone()).or_insert(0) += event.count;
    }

    // Busiest = max total; iterate the ordered keys so ties keep the smallest source.
    let busiest = by_source
        .iter()
        .max_by_key(|(source, count)| (**count, std::cmp::Reverse((*source).clone())))
        .map(|(source, _)| source.clone());

    Rollup {
        total,
        by_source,
        busiest,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(source: &str, count: u64) -> Event {
        Event {
            source: source.to_string(),
            count,
        }
    }

    #[test]
    fn sums_and_groups_by_source() {
        let rollup = roll_up(&[ev("api", 3), ev("web", 5), ev("api", 4)]);
        assert_eq!(rollup.total, 12);
        assert_eq!(rollup.by_source.get("api"), Some(&7));
        assert_eq!(rollup.by_source.get("web"), Some(&5));
        assert_eq!(rollup.busiest.as_deref(), Some("api"));
    }

    #[test]
    fn is_order_independent() {
        let a = roll_up(&[ev("api", 3), ev("web", 5), ev("api", 4)]);
        let b = roll_up(&[ev("api", 4), ev("api", 3), ev("web", 5)]);
        assert_eq!(a, b);
    }

    #[test]
    fn ties_break_to_smallest_source() {
        // `api` and `web` tie at 5; the smaller name wins deterministically.
        let rollup = roll_up(&[ev("web", 5), ev("api", 5)]);
        assert_eq!(rollup.busiest.as_deref(), Some("api"));
    }

    #[test]
    fn empty_has_no_busiest() {
        let rollup = roll_up(&[]);
        assert_eq!(rollup.total, 0);
        assert!(rollup.by_source.is_empty());
        assert_eq!(rollup.busiest, None);
    }
}
