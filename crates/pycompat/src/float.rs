//! Float text form, decimal rounding, and compensated summation, with exact results.

use std::fmt::Write;

const MAX_ROUND_DIGITS: u32 = 323;

/// Formats `x` with the fewest digits that parse back to `x`, in exponent
/// form below 1e-4 and from 1e16.
///
/// # Panics
/// Panics if Rust's own finite float formatting does not expose the expected fields.
#[must_use]
pub fn float_repr(x: f64) -> String {
    if x.is_nan() {
        return "nan".to_owned();
    }
    let sign = if x.is_sign_negative() { "-" } else { "" };
    if x.is_infinite() {
        return format!("{sign}inf");
    }
    if x == 0.0 {
        return format!("{sign}0.0");
    }

    let text = format!("{:e}", x.abs());
    let (mantissa, exponent_text) = text
        .split_once('e')
        .expect("finite nonzero f64 scientific formatting always contains an exponent");
    let mut digits: String = mantissa.chars().filter(char::is_ascii_digit).collect();
    while digits.ends_with('0') {
        digits.pop();
    }
    let e: i32 = exponent_text
        .parse()
        .expect("exponent from f64 scientific formatting is a valid i32");

    let mut out = sign.to_owned();
    if !(-4..16).contains(&e) {
        append_scientific(&mut out, &digits, e);
        return out;
    }

    append_positional(&mut out, &digits, e);
    out
}

/// Rounds the exact value of `x` to `ndigits` decimal places, with an exact
/// tie going to the even digit.
///
/// # Panics
/// Panics if a decimal string produced by Rust's own float formatting does not parse.
#[must_use]
pub fn round_half_even(x: f64, ndigits: u32) -> f64 {
    // No rounding can change an f64 above 323 digits, and this bounds the format.
    if !x.is_finite() || ndigits > MAX_ROUND_DIGITS {
        return x;
    }

    let precision = usize::try_from(ndigits).expect("ndigits <= MAX_ROUND_DIGITS");
    format!("{x:.precision$}")
        .parse()
        .expect("Rust's own exactly rounded float formatting parses back to f64")
}

/// Sums `values` with Neumaier compensation, starting from 0.0.
#[must_use]
pub fn sum_builtin(values: &[f64]) -> f64 {
    let mut result = 0.0f64;
    let mut c = 0.0f64;

    for x in values {
        let t = result + x;
        if result.abs() >= x.abs() {
            c += (result - t) + x;
        } else {
            c += (x - t) + result;
        }
        result = t;
    }

    if c != 0.0 && c.is_finite() {
        result += c;
    }

    result
}

fn append_positional(out: &mut String, digits: &str, e: i32) {
    if e >= 0 {
        let integer_len =
            usize::try_from(e).expect("nonnegative decimal exponent is at most i32::MAX") + 1;
        if integer_len >= digits.len() {
            out.push_str(digits);
            for _ in 0..integer_len - digits.len() {
                out.push('0');
            }
            out.push('.');
            out.push('0');
        } else {
            let (integer, fraction) = digits.split_at(integer_len);
            out.push_str(integer);
            out.push('.');
            out.push_str(fraction);
        }
    } else {
        out.push_str("0.");
        let leading_zeros = usize::try_from(e.unsigned_abs())
            .expect("negative decimal exponent magnitude is at most i32::MAX")
            - 1;
        for _ in 0..leading_zeros {
            out.push('0');
        }
        out.push_str(digits);
    }
}

fn append_scientific(out: &mut String, digits: &str, e: i32) {
    if digits.len() > 1 {
        let (first, rest) = digits.split_at(1);
        out.push_str(first);
        out.push('.');
        out.push_str(rest);
    } else {
        out.push_str(digits);
    }

    write!(out, "e{e:+03}").expect("writing to a String cannot fail");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::{bits, cases, float, floats};

    #[test]
    fn test_float_repr() {
        let all: Vec<(String, String)> = cases("float", "float_repr");
        for (x, expected) in all {
            assert_eq!(float_repr(float(&x)), expected, "x: {x}");
        }
    }

    #[test]
    fn test_round_half_even() {
        let all: Vec<(String, u32, String)> = cases("float", "round_half_even");
        for (x, ndigits, expected) in all {
            let got = round_half_even(float(&x), ndigits);
            assert_eq!(got.to_bits(), bits(&expected), "x: {x}, ndigits: {ndigits}");
        }
    }

    #[test]
    fn test_sum_builtin() {
        let all: Vec<(Vec<String>, String)> = cases("float", "sum_builtin");
        for (values, expected) in all {
            let got = sum_builtin(&floats(&values));
            assert_eq!(got.to_bits(), bits(&expected), "values: {values:?}");
        }
    }
}
