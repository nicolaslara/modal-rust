//! `examples/error-handling` — how a failure crosses the Modal boundary.
//!
//! Teaching ONE concept: a `#[modal_rust::function]` can fail two ways, and the
//! ERROR TYPE you return decides what the caller gets back.
//!
//! - Return `anyhow::Result<_>` → an OPAQUE error. The caller sees a human-readable
//!   `message` (the full anyhow chain) and `details = null`. Good for "just bubble
//!   it up".
//! - Return `Result<_, YourError>` where `YourError: Serialize` → a STRUCTURED
//!   error. The caller still sees the `message`, AND a machine-readable `details`
//!   object (your serialized error) it can branch on — a code, a field, anything.
//!
//! Both land on the same frozen failure kind (`function_error`); the only
//! difference is whether `details` carries your typed error. The companion
//! `src/bin/error_handling.rs` runs both offline and prints what the caller sees.
//!
//! The shared withdrawal logic (validation + arithmetic) lives in [`account`] so
//! this file stays a clean surface: the input/output types plus the two
//! `#[function]`s, which differ ONLY in how they shape the same failure.

use modal_rust::function;
use serde::{Deserialize, Serialize};

pub mod account;

use account::{apply_withdrawal, WithdrawalFailure};

/// The receipt a successful withdrawal returns.
#[derive(Debug, Serialize, Deserialize)]
pub struct Receipt {
    /// Amount actually withdrawn.
    pub withdrawn: i64,
    /// Balance remaining after the withdrawal.
    pub remaining: i64,
}

/// Withdraw `amount` against `balance`, reporting failure as a PLAIN `anyhow` error.
///
/// Because the return type is `anyhow::Result<_>`, the user error is OPAQUE: the
/// caller gets the `message` (the anyhow chain) and `details = null`. This is the
/// "bubble it up, no machine-readable shape" path.
#[function]
pub fn withdraw(amount: i64, balance: i64) -> anyhow::Result<Receipt> {
    match apply_withdrawal(amount, balance) {
        Ok((withdrawn, remaining)) => Ok(Receipt {
            withdrawn,
            remaining,
        }),
        // The shared failure is flattened into an opaque human-readable string —
        // nothing machine-readable rides along the `anyhow` path.
        Err(WithdrawalFailure::NonPositive { amount }) => {
            anyhow::bail!("amount must be positive, got {amount}")
        }
        Err(WithdrawalFailure::InsufficientFunds { .. }) => {
            anyhow::bail!("insufficient funds: asked {amount}, have {balance}")
        }
    }
}

/// A STRUCTURED, machine-readable error. Deriving `Serialize` is the whole trick:
/// the macro detects the `Serialize` impl and puts `serde_json::to_value(&e)` into
/// the envelope's `details`, so the caller can branch on the typed shape instead of
/// scraping a string.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum WithdrawError {
    /// Requested a non-positive amount; carries the offending value.
    NonPositive {
        /// The non-positive amount that was requested.
        amount: i64,
    },
    /// Not enough balance; carries the shortfall so the caller can act on it.
    InsufficientFunds {
        /// How much more balance would have been needed.
        shortfall: i64,
    },
}

impl std::fmt::Display for WithdrawError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WithdrawError::NonPositive { amount } => {
                write!(f, "amount must be positive, got {amount}")
            }
            WithdrawError::InsufficientFunds { shortfall } => {
                write!(f, "insufficient funds: short by {shortfall}")
            }
        }
    }
}

impl std::error::Error for WithdrawError {}

/// Carry the shared [`WithdrawalFailure`] onto the wire-facing `Serialize` shape.
/// The two variants line up one-to-one, so this is the whole "structured" path:
/// the failure the policy produced becomes the typed `details` the caller branches on.
impl From<WithdrawalFailure> for WithdrawError {
    fn from(failure: WithdrawalFailure) -> Self {
        match failure {
            WithdrawalFailure::NonPositive { amount } => WithdrawError::NonPositive { amount },
            WithdrawalFailure::InsufficientFunds { shortfall } => {
                WithdrawError::InsufficientFunds { shortfall }
            }
        }
    }
}

/// Withdraw `amount` against `balance`, reporting failure as a STRUCTURED
/// `Serialize` error.
///
/// Same logic as [`withdraw`] — both call [`account::apply_withdrawal`] — but the
/// error type is [`WithdrawError`] (a `Serialize` enum) instead of `anyhow::Error`.
/// The caller now gets the same `message` PLUS a machine-readable `details` object it
/// can match on.
#[function]
pub fn withdraw_checked(amount: i64, balance: i64) -> Result<Receipt, WithdrawError> {
    let (withdrawn, remaining) = apply_withdrawal(amount, balance)?;
    Ok(Receipt {
        withdrawn,
        remaining,
    })
}
