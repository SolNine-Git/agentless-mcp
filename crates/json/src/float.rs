//! Float text form and decimal rounding, with exact results.

use std::fmt::Write;

const MAX_ROUND_DIGITS: u32 = 323;
const GENERAL_DIGITS: usize = 6;
// The general form is positional below this decimal exponent, the digit count.
const GENERAL_EXPONENT_LIMIT: i32 = 6;

/// Formats `x` with the fewest digits that parse back to `x`, in exponent
/// form below 1e-4 and from 1e16.
///
/// # Panics
/// Panics if Rust's own finite float formatting does not expose the expected fields.
#[must_use]
pub fn float_text(x: f64) -> String {
    if let Some(text) = non_finite(x) {
        return text;
    }
    let sign = if x.is_sign_negative() { "-" } else { "" };
    if x == 0.0 {
        return format!("{sign}0.0");
    }

    // The shortest form gives the digit count. It rounds an exact tie up, and the exact form at
    // that count rounds a tie to the even digit.
    let shortest = format!("{:e}", x.abs());
    let count = shortest.split('e').next().map_or(0, |mantissa| {
        mantissa.bytes().filter(u8::is_ascii_digit).count()
    });
    let text = format!("{:.*e}", count.saturating_sub(1), x.abs());
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

/// Formats `x` with `places` digits after the point, its exact value rounded half to even. NaN
/// is "nan" and an infinity is "inf" or "-inf".
#[must_use]
pub fn fixed(x: f64, places: usize) -> String {
    non_finite(x).unwrap_or_else(|| format!("{x:.places$}"))
}

/// Formats `x` with six significant digits and no trailing zeros: positional from 1e-4 to below
/// 1e6, exponent form outside. NaN is "nan" and an infinity is "inf" or "-inf".
///
/// # Panics
/// Panics if Rust's own finite float formatting does not expose the expected fields.
#[must_use]
pub fn general(x: f64) -> String {
    if let Some(text) = non_finite(x) {
        return text;
    }
    // One exact rounding gives the six digits and the exponent after rounding, so 999999.5 lays
    // out as 1e+06. The layout below never rounds again.
    let text = format!("{:.*e}", GENERAL_DIGITS - 1, x.abs());
    let (mantissa, exponent_text) = text
        .split_once('e')
        .expect("finite f64 scientific formatting always contains an exponent");
    let digits: String = mantissa.chars().filter(char::is_ascii_digit).collect();
    let digits = digits.trim_end_matches('0');
    let e: i32 = exponent_text
        .parse()
        .expect("exponent from f64 scientific formatting is a valid i32");

    let mut out = if x.is_sign_negative() { "-" } else { "" }.to_owned();
    if digits.is_empty() {
        out.push('0');
    } else if (-4..GENERAL_EXPONENT_LIMIT).contains(&e) {
        append_plain(&mut out, digits, e);
    } else {
        append_scientific(&mut out, digits, e);
    }
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

fn non_finite(x: f64) -> Option<String> {
    if x.is_nan() {
        return Some("nan".to_owned());
    }
    x.is_infinite()
        .then(|| if x < 0.0 { "-inf" } else { "inf" }.to_owned())
}

// The digits laid out at decimal exponent `e`, with no point when no fraction digit is left.
fn append_plain(out: &mut String, digits: &str, e: i32) {
    if e < 0 {
        out.push_str("0.");
        for _ in 1..e.unsigned_abs() {
            out.push('0');
        }
        out.push_str(digits);
        return;
    }
    let integer_len = usize::try_from(e).expect("nonnegative decimal exponent fits usize") + 1;
    if integer_len >= digits.len() {
        out.push_str(digits);
        out.extend(std::iter::repeat_n('0', integer_len - digits.len()));
    } else {
        let (integer, fraction) = digits.split_at(integer_len);
        out.push_str(integer);
        out.push('.');
        out.push_str(fraction);
    }
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
    use text::testing::{bits, cases, float};

    #[test]
    fn test_float_text() {
        let all: Vec<(String, String)> = cases(env!("CARGO_MANIFEST_DIR"), "float", "float_repr");
        for (x, expected) in all {
            assert_eq!(float_text(float(&x)), expected, "x: {x}");
        }
    }

    // Exact decimal ties: 2247601717616233.25, -3906101348481.40625 and 1761169953.31640625.
    #[test]
    fn float_text_of_an_exact_tie_matches_repr() {
        let shown = [
            0x431f_f0ba_37e6_79a5,
            0xc28c_6bae_4444_0b40,
            0x41da_3e54_8854_4000,
        ]
        .map(|bits| float_text(f64::from_bits(bits)));
        let expected = [
            "2247601717616233.2",
            "-3906101348481.4062",
            "1761169953.3164062",
        ];
        assert_eq!(shown, expected);
    }

    #[test]
    fn test_round_half_even() {
        let all: Vec<(String, u32, String)> =
            cases(env!("CARGO_MANIFEST_DIR"), "float", "round_half_even");
        for (x, ndigits, expected) in all {
            let got = round_half_even(float(&x), ndigits);
            assert_eq!(got.to_bits(), bits(&expected), "x: {x}, ndigits: {ndigits}");
        }
    }

    #[test]
    fn fixed_matches_the_reference_cases() {
        let all: Vec<(String, usize, String)> =
            cases(env!("CARGO_MANIFEST_DIR"), "format/format", "fixed");
        assert!(!all.is_empty());
        for (x, places, expected) in all {
            assert_eq!(
                fixed(float(&x), places),
                expected,
                "x: {x}, places: {places}"
            );
        }
    }

    #[test]
    fn general_matches_the_reference_cases() {
        let all: Vec<(String, String)> =
            cases(env!("CARGO_MANIFEST_DIR"), "format/format", "general");
        assert!(!all.is_empty());
        for (x, expected) in all {
            assert_eq!(general(float(&x)), expected, "x: {x}");
        }
    }
}
