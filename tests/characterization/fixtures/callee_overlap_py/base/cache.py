from helpers import c_apply, c_diff, c_fetch, c_flush, c_log



def sync_cache(key):
    fresh = c_fetch(key)
    delta = c_diff(fresh)
    applied = c_apply(delta)
    c_log(applied)
    return c_flush(applied)
