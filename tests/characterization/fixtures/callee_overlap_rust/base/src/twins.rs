use crate::helpers::{m_join, m_left, m_right, m_tag, r_end, r_mix, r_seed, r_step};


pub fn merge_b(pairs: &[(i64, i64)]) -> i64 {
    let mut joined = 0;
    for (left, right) in pairs {
        joined += m_join(m_left(*left) - m_right(*right));
    }
    m_tag(joined)
}

pub fn fold_y(values: &[i64]) -> i64 {
    values
        .iter()
        .map(|value| r_step(r_seed(*value)))
        .fold(0, |sum, value| r_end(r_mix(sum + value)))
}
