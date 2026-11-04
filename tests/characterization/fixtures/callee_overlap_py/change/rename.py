from helpers import r_end, r_mix, r_seed, r_step


def fold_z(value):
    seed = r_seed(value)
    return r_end(r_mix(r_step(seed)))
