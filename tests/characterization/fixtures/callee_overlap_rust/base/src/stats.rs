use crate::helpers::{stat_high, stat_low, stat_mean, stat_spread};


pub fn summarize(values: &[i64]) -> i64 {
    let mean = stat_mean(values.len() as i64);
    let spread = stat_spread(mean);
    stat_low(spread) + stat_high(spread)
}
