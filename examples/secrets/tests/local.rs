//! Offline proof (zero Modal, zero network) that `check_secret` does a REAL
//! `std::env::var` read: when the secret's env var is present the report says so and
//! carries the value's TRUE length; when it is absent the report says absent with
//! length zero. The length is the real `value.len()`, never a fixed constant and
//! never the value itself — so this both proves the read is real and proves it never
//! leaks the secret.
//!
//! These assertions mutate process env vars, so they run SERIALLY within this single
//! `#[test]` to avoid racing each other (Cargo runs separate `#[test]`s on parallel
//! threads that share one process environment).

use example_secrets::{check_secret, Request};
use modal_rust::App;

#[test]
fn check_secret_reports_the_real_presence_and_length() {
    // ----- Present: the attached secret's env var is set. -----
    // A value whose length is its OWN, not a fixed constant — proves `len` is the
    // real `value.len()`. The body reports presence + length but never the value, so
    // the secret is read for real yet never leaks.
    let value = "sk-test-0123456789"; // 18 chars
    std::env::set_var("MY_API_KEY", value);

    // Through the facade's local path (your `Request` in, your `Report` back).
    let report: example_secrets::Report = App::local()
        .function("check_secret")
        .local(Request {})
        .unwrap();
    assert!(report.present, "the set env var is read as present");
    assert_eq!(
        report.len,
        value.len(),
        "len is the REAL value length, not a fixed constant"
    );

    // A DIFFERENT value yields a DIFFERENT length — the read is genuinely of the
    // environment, not a hard-coded number.
    let longer = "sk-test-0123456789-extra"; // 24 chars
    std::env::set_var("MY_API_KEY", longer);
    let report2 = check_secret(Request {}).unwrap();
    assert!(report2.present);
    assert_eq!(report2.len, longer.len());
    assert_ne!(
        report.len, report2.len,
        "different secret values report different lengths"
    );

    // The report never carries the value itself — `Report` exposes only `present`
    // and `len`, so reading `report2.len` is the ONLY thing the value's length can be
    // observed through (the secret itself is never echoed back).
    assert!(
        report2.len > 0,
        "the value is non-empty yet only its length escapes"
    );

    // ----- Absent: with no secret attached, the read finds nothing. -----
    std::env::remove_var("MY_API_KEY");
    let absent = check_secret(Request {}).unwrap();
    assert!(!absent.present, "an unset env var reads as absent");
    assert_eq!(absent.len, 0, "absent reports length zero");
}
