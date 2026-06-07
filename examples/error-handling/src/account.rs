//! The real withdrawal computation, kept off the modal surface in `lib.rs`.
//!
//! `lib.rs` owns the input/output types and the two `#[modal_rust::function]`s; this
//! module owns the one piece of shared logic both of them run: validate the request
//! and do the arithmetic. The work is small, CPU-only, and fully deterministic.
//!
//! Both `withdraw` (which surfaces failure as an opaque `anyhow` error) and
//! `withdraw_checked` (which surfaces it as a structured `Serialize` error) call
//! [`apply_withdrawal`]; they differ ONLY in how they shape the failure for the
//! caller, never in what counts as a failure.

/// Why a withdrawal was rejected. A single shared failure type so both functions
/// validate identically; each function maps it onto the error SHAPE it advertises.
#[derive(Debug, PartialEq, Eq)]
pub enum WithdrawalFailure {
    /// The requested amount was zero or negative; carries the offending value.
    NonPositive {
        /// The non-positive amount that was requested.
        amount: i64,
    },
    /// The balance could not cover the request; carries the shortfall so the caller
    /// can act on it (e.g. prompt a top-up).
    InsufficientFunds {
        /// How much more balance would have been needed.
        shortfall: i64,
    },
}

/// Withdraw `amount` against `balance`, returning `(withdrawn, remaining)` on success.
///
/// The whole policy lives here so the two modal functions can't drift apart:
///
/// - A non-positive `amount` is rejected with [`WithdrawalFailure::NonPositive`].
/// - An `amount` larger than `balance` is rejected with
///   [`WithdrawalFailure::InsufficientFunds`], carrying the exact shortfall.
/// - Otherwise the withdrawal succeeds: `withdrawn == amount` and
///   `remaining == balance - amount`.
///
/// # Examples
///
/// ```
/// use example_error_handling::account::{apply_withdrawal, WithdrawalFailure};
/// assert_eq!(apply_withdrawal(40, 100), Ok((40, 60)));
/// assert_eq!(apply_withdrawal(100, 100), Ok((100, 0))); // exact balance is fine
/// assert_eq!(
///     apply_withdrawal(150, 100),
///     Err(WithdrawalFailure::InsufficientFunds { shortfall: 50 })
/// );
/// assert_eq!(
///     apply_withdrawal(-1, 100),
///     Err(WithdrawalFailure::NonPositive { amount: -1 })
/// );
/// ```
pub fn apply_withdrawal(amount: i64, balance: i64) -> Result<(i64, i64), WithdrawalFailure> {
    if amount <= 0 {
        return Err(WithdrawalFailure::NonPositive { amount });
    }
    if amount > balance {
        return Err(WithdrawalFailure::InsufficientFunds {
            shortfall: amount - balance,
        });
    }
    Ok((amount, balance - amount))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_withdrawal_returns_withdrawn_and_remaining() {
        assert_eq!(apply_withdrawal(40, 100), Ok((40, 60)));
        assert_eq!(apply_withdrawal(100, 100), Ok((100, 0))); // exact balance is allowed
    }

    #[test]
    fn non_positive_amount_is_rejected() {
        assert_eq!(
            apply_withdrawal(0, 100),
            Err(WithdrawalFailure::NonPositive { amount: 0 })
        );
        assert_eq!(
            apply_withdrawal(-5, 100),
            Err(WithdrawalFailure::NonPositive { amount: -5 })
        );
    }

    #[test]
    fn over_withdrawal_reports_the_exact_shortfall() {
        assert_eq!(
            apply_withdrawal(150, 100),
            Err(WithdrawalFailure::InsufficientFunds { shortfall: 50 })
        );
    }
}
