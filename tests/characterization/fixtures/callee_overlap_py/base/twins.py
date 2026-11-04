from helpers import m_join, m_left, m_right, m_tag, r_end, r_mix, r_seed, r_step



def merge_b(pairs):
    joined = 0
    for left, right in pairs:
        joined += m_join(m_left(left) - m_right(right))
    return m_tag(joined)


def fold_y(values):
    total = 0
    for value in values:
        total = r_end(r_mix(total + r_step(r_seed(value))))
    return total
