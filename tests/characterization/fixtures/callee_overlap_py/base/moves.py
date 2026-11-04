from helpers import m_join, m_left, m_right, m_tag



def merge_a(left, right):
    joined = m_join(m_left(left) + m_right(right))
    return m_tag(joined)


def keep():
    return 1
