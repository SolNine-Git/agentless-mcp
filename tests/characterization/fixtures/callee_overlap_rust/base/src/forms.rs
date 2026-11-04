use crate::helpers::{v_check, v_coerce, v_report, v_trim};


pub fn validate_form(field: i64) -> i64 {
    let trimmed = v_trim(field);
    if v_check(trimmed) == 0 {
        return v_report(trimmed);
    }
    v_coerce(trimmed)
}
