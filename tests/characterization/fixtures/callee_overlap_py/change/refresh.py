from helpers import c_apply, c_diff, c_fetch, c_flush



def refresh_cache(keys):
    total = 0
    for key in keys:
        delta = c_diff(c_fetch(key))
        if delta == 0:
            continue
        total += c_apply(delta)
    return c_flush(total)
