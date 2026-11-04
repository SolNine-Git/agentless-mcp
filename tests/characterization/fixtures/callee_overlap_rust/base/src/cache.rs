use crate::helpers::{c_apply, c_diff, c_fetch, c_flush, c_log};


pub fn sync_cache(key: i64) -> i64 {
    let fresh = c_fetch(key);
    let delta = c_diff(fresh);
    let applied = c_apply(delta);
    c_log(applied);
    c_flush(applied)
}
