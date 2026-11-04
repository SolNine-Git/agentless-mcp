use crate::helpers::{r_end, r_mix, r_seed, r_step};

pub fn fold_z(value: i64) -> i64 {
    let seed = r_seed(value);
    r_end(r_mix(r_step(seed)))
}
