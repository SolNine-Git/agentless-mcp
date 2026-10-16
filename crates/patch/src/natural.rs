//! Whole numbers of any size, for the line numbers and counts of a hunk header.

use std::cmp::Ordering;
use std::fmt;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Natural {
    // The decimal digit values, the most significant first, without leading zeros; zero has none.
    digits: Vec<u8>,
}

impl Natural {
    pub(crate) fn one() -> Natural {
        Natural { digits: vec![1] }
    }

    fn from_digits(values: impl IntoIterator<Item = u8>) -> Natural {
        let digits = values.into_iter().skip_while(|&digit| digit == 0).collect();
        Natural { digits }
    }

    fn from_usize(value: usize) -> Natural {
        Natural::from_digits(value.to_string().bytes().map(|byte| byte - b'0'))
    }

    pub(crate) fn is_zero(&self) -> bool {
        self.digits.is_empty()
    }

    // The value as a usize, or `None` when it is larger than any usize.
    pub(crate) fn to_usize(&self) -> Option<usize> {
        self.digits.iter().try_fold(0_usize, |value, &digit| {
            value.checked_mul(10)?.checked_add(usize::from(digit))
        })
    }

    pub(crate) fn plus(&self, other: &Natural) -> Natural {
        let (mut left, mut right) = (self.digits.iter().rev(), other.digits.iter().rev());
        let mut sum = Vec::with_capacity(self.digits.len().max(other.digits.len()) + 1);
        let mut carry = 0;
        loop {
            let (a, b) = (left.next(), right.next());
            if a.is_none() && b.is_none() && carry == 0 {
                break;
            }
            let total = a.copied().unwrap_or(0) + b.copied().unwrap_or(0) + carry;
            sum.push(total % 10);
            carry = total / 10;
        }
        Natural::from_digits(sum.into_iter().rev())
    }

    // The difference, for a `value` that is at most this number.
    pub(crate) fn minus(&self, value: usize) -> Natural {
        let other = Natural::from_usize(value);
        let mut right = other.digits.iter().rev();
        let mut borrow = 0;
        let mut difference = Vec::with_capacity(self.digits.len());
        for &digit in self.digits.iter().rev() {
            let taken = right.next().copied().unwrap_or(0) + borrow;
            let raised = digit + 10 - taken;
            borrow = u8::from(raised < 10);
            difference.push(raised % 10);
        }
        Natural::from_digits(difference.into_iter().rev())
    }
}

impl Ord for Natural {
    fn cmp(&self, other: &Self) -> Ordering {
        self.digits
            .len()
            .cmp(&other.digits.len())
            .then_with(|| self.digits.cmp(&other.digits))
    }
}

impl PartialOrd for Natural {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl fmt::Display for Natural {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.digits.is_empty() {
            return f.write_str("0");
        }
        self.digits
            .iter()
            .try_for_each(|&digit| write!(f, "{}", char::from(b'0' + digit)))
    }
}

// The number that the decimal digits at the start of `text` spell, and the text after them, or
// `None` when `text` does not start with a digit. A digit is any Unicode decimal digit.
pub(crate) fn leading(text: &str) -> Option<(Natural, &str)> {
    let end = text
        .char_indices()
        .find(|&(_, c)| !text::is_decimal(c))
        .map_or(text.len(), |(at, _)| at);
    if end == 0 {
        return None;
    }
    let number = Natural::from_digits(text[..end].chars().filter_map(digit_value));
    Some((number, &text[end..]))
}

// Unicode encodes each set of decimal digits as 10 consecutive code points from 0 to 9, so a
// digit's value is its distance from the start of its run, modulo 10.
fn digit_value(c: char) -> Option<u8> {
    if !text::is_decimal(c) {
        return None;
    }
    let code = u32::from(c);
    let start = (0..code)
        .rev()
        .take_while(|&before| char::from_u32(before).is_some_and(text::is_decimal))
        .last()
        .unwrap_or(code);
    u8::try_from((code - start) % 10).ok()
}

#[cfg(test)]
mod tests {
    use serde::Deserialize;

    use super::*;

    #[derive(Deserialize)]
    struct Digits {
        digits: String,
        values: String,
    }

    #[test]
    fn every_decimal_digit_has_the_reference_value() {
        let [table]: [Digits; 1] =
            text::testing::cases(env!("CARGO_MANIFEST_DIR"), "unidiff", "decimal_digits");
        let values: Vec<Option<u8>> = table.values.bytes().map(|byte| Some(byte - b'0')).collect();
        let found: Vec<Option<u8>> = table.digits.chars().map(digit_value).collect();
        assert_eq!(found, values);
        let every: String = (0..=0x10_ffff)
            .filter_map(char::from_u32)
            .filter(|&c| text::is_decimal(c))
            .collect();
        assert_eq!(every, table.digits);
    }

    #[test]
    fn arithmetic_carries_and_borrows_across_digits() {
        let nines = leading("999").map(|(number, _)| number).unwrap_or_default();
        assert_eq!(nines.plus(&Natural::one()).to_string(), "1000");
        assert_eq!(nines.plus(&Natural::one()).minus(1).to_string(), "999");
        assert_eq!(nines.minus(999).to_string(), "0");
        assert!(nines.minus(999).is_zero());
        assert!(Natural::from_usize(1000) > nines);
        let huge = leading("18446744073709551616").map(|(number, _)| number);
        assert_eq!(huge.and_then(|number| number.to_usize()), None);
        assert_eq!(nines.to_usize(), Some(999));
    }

    #[test]
    fn leading_reads_only_the_digits_at_the_start() {
        let read = leading("007,3 +x").map(|(number, rest)| (number.to_string(), rest));
        assert_eq!(read, Some(("7".to_owned(), ",3 +x")));
        assert!(leading(",3").is_none());
        assert!(leading("").is_none());
    }
}
