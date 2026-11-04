use crate::helpers::{exp_close, exp_escape, exp_open, exp_row};


pub fn export_rows(rows: &[i64]) -> i64 {
    let mut out = exp_open(0);
    for row in rows {
        out += exp_row(exp_escape(*row));
    }
    exp_close(out)
}
