//! The real persistence computation, kept off the modal surface in `lib.rs`.
//!
//! `lib.rs` owns the input/output structs and the `#[modal_rust::function]`; this
//! module owns the actual filesystem work so the surface reads as nothing but "your
//! struct in, your struct out". The work IS the lesson here: a real `std::fs` append
//! followed by a real read-back and line count. It is deterministic given the file's
//! prior contents, and — because it takes the volume mount directory as a parameter —
//! the offline test can point it at a temp dir instead of the live `/data` mount.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;

/// Append one visit line to `<dir>/visits.log`, then read the whole log back and
/// return how many lines (visits) it now holds.
///
/// This is real, observable persistence: the line written by an earlier call is
/// still in the file when a later call reads it back, so the returned count grows by
/// one each call against the same directory. On a live Modal run `dir` is the volume
/// mount (`/data`), so the file — and therefore the count — survives across calls and
/// even across fresh containers.
///
/// `dir` is created if missing so the very first call against a freshly attached
/// (empty) volume has somewhere to write. The body is plain `std::fs`; the durability
/// comes entirely from the volume the decorator mounts at `dir`.
///
/// # Examples
///
/// ```
/// # use std::fs;
/// use example_volumes::visit_log::record;
/// let dir = std::env::temp_dir().join(format!("mr-volumes-doctest-{}", std::process::id()));
/// let _ = fs::remove_dir_all(&dir); // start from a fresh "volume"
/// assert_eq!(record(&dir, "first").unwrap(), 1); //  fresh log -> 1 line
/// assert_eq!(record(&dir, "second").unwrap(), 2); // first line survived -> 2 lines
/// assert_eq!(record(&dir, "third").unwrap(), 3); //  and again -> 3 lines
/// fs::remove_dir_all(&dir).unwrap();
/// ```
pub fn record(dir: &Path, label: &str) -> std::io::Result<usize> {
    // Create the mount dir if the volume is freshly attached so the first call has
    // somewhere to write.
    std::fs::create_dir_all(dir)?;
    let log = dir.join("visits.log");

    // Append one line for this visit.
    let mut f = OpenOptions::new().create(true).append(true).open(&log)?;
    writeln!(f, "{label}")?;
    drop(f);

    // Read the persisted log back: its line count is the running visit total. On a
    // second call it includes the first call's line — that is the persistence proof.
    let contents = std::fs::read_to_string(&log)?;
    Ok(contents.lines().count())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("mr-volumes-{tag}-{}", std::process::id()))
    }

    #[test]
    fn count_grows_by_one_per_call_against_the_same_dir() {
        let dir = scratch("grows");
        let _ = std::fs::remove_dir_all(&dir);

        // Each call sees the previous calls' lines: the file persists between calls,
        // exactly as a mounted volume would persist it between Modal invocations.
        assert_eq!(record(&dir, "a").unwrap(), 1);
        assert_eq!(record(&dir, "b").unwrap(), 2);
        assert_eq!(record(&dir, "c").unwrap(), 3);

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn writes_each_label_once_in_order() {
        let dir = scratch("labels");
        let _ = std::fs::remove_dir_all(&dir);

        record(&dir, "alpha").unwrap();
        record(&dir, "beta").unwrap();
        let contents = std::fs::read_to_string(dir.join("visits.log")).unwrap();
        assert_eq!(contents.lines().collect::<Vec<_>>(), ["alpha", "beta"]);

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn creates_a_missing_dir_on_the_first_call() {
        // A freshly attached, empty volume: the mount dir does not exist yet.
        let dir = scratch("fresh");
        let _ = std::fs::remove_dir_all(&dir);
        assert!(!dir.exists());

        assert_eq!(record(&dir, "only").unwrap(), 1);
        assert!(dir.join("visits.log").exists());

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
