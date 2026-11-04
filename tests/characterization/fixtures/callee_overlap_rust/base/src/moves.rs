use crate::helpers::{m_join, m_left, m_right, m_tag};


pub fn merge_a(left: i64, right: i64) -> i64 {
    let joined = m_join(m_left(left) + m_right(right));
    m_tag(joined)
}

pub fn keep() -> i64 {
    1
}
