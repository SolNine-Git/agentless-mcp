def s_base(value):
    return value * 8

def s_bonus(value):
    return value * 9

def s_cap(value):
    return value * 10

def s_round(value):
    return value * 11

def p_left(value):
    return value * 12

def p_right(value):
    return value * 13


def score(points):
    raw = s_base(points)
    return s_round(s_cap(s_bonus(raw)))


def pair_up(value):
    return p_left(value) + p_right(value)
