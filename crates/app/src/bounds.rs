//! The numeric bounds that every read operation checks, with one refusal text per rule. Both
//! doors share them: a bound is checked by the operation, never by one door.

use crate::{Error, IntArg};

/// Returns `value`, saturated at `usize::MAX`, when it is at least `low`.
///
/// # Errors
///
/// Returns an operation failure that names `name` when `value` is below `low`.
pub fn at_least(value: &IntArg, low: usize, name: &str) -> Result<usize, Error> {
    value
        .value()
        .filter(|&number| number >= low)
        .ok_or_else(|| {
            let message = format!("{name} must be at least {low}, got {value}");
            kernel::Error::OperationFailed(message).into()
        })
}

/// Returns `value` when it lies from `low` through `high`, both included.
///
/// # Errors
///
/// Returns an operation failure that names `name` when `value` lies outside the range.
pub fn within(value: &IntArg, low: usize, high: usize, name: &str) -> Result<usize, Error> {
    let inside = value.value().filter(|number| (low..=high).contains(number));
    inside.ok_or_else(|| {
        let message = format!("{name} takes a value from {low} through {high}, got {value}");
        kernel::Error::OperationFailed(message).into()
    })
}

/// Returns `value` when it lies from `low` through `high`, both included. NaN lies in no range.
///
/// # Errors
///
/// Returns an operation failure that names `name` when `value` lies outside the range.
pub fn within_float(value: f64, low: f64, high: f64, name: &str) -> Result<f64, Error> {
    if (low..=high).contains(&value) {
        return Ok(value);
    }
    let [low, high, value] = [low, high, value].map(json::float_text);
    let message = format!("{name} takes a value from {low} through {high}, got {value}");
    Err(kernel::Error::OperationFailed(message).into())
}

#[cfg(test)]
mod tests;
