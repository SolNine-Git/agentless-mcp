use crate::helpers::{v_check, v_coerce, v_report, v_trim};


pub fn screen_form(fields: &[i64]) -> i64 {
    fields
        .iter()
        .map(|field| v_coerce(v_trim(*field)))
        .filter(|value| v_check(*value) != 0)
        .fold(0, |sum, value| sum + v_report(value))
}
