//! Float summation with a compensation term.

/// Sums `values` with Neumaier compensation, starting from 0.0.
#[must_use]
pub fn compensated_sum(values: &[f64]) -> f64 {
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

#[cfg(test)]
mod tests {
    use super::*;
    use text::testing::{bits, cases, floats};

    #[test]
    fn compensated_sum_cases() {
        let all: Vec<(Vec<String>, String)> =
            cases(env!("CARGO_MANIFEST_DIR"), "sum", "sum_builtin");
        for (values, expected) in all {
            let got = compensated_sum(&floats(&values));
            let want = f64::from_bits(bits(&expected));
            // The CPU sets the sign of a NaN that an operation creates, and no output shows it.
            if want.is_nan() {
                assert!(got.is_nan(), "values: {values:?}");
            } else {
                assert_eq!(got.to_bits(), want.to_bits(), "values: {values:?}");
            }
        }
    }
}
