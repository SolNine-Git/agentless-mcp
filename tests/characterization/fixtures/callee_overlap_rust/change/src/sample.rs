use crate::helpers::{stat_high, stat_low, stat_mean, stat_spread};


pub fn describe_sample(sample: &[i64]) -> i64 {
    let center = stat_mean(sample.len() as i64 + 1);
    let width = stat_spread(center);
    stat_low(width) * 2 + stat_high(width)
}
