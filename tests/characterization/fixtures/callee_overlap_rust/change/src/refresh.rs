use crate::helpers::{c_apply, c_diff, c_fetch, c_flush};


pub fn refresh_cache(keys: &[i64]) -> i64 {
    let mut total = 0;
    for key in keys {
        let delta = c_diff(c_fetch(*key));
        if delta == 0 {
            continue;
        }
        total += c_apply(delta);
    }
    c_flush(total)
}
