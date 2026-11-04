//! Small helpers that the planted functions call across files.

pub fn inv_header(value: i64) -> i64 {
    value + 0
}

pub fn inv_lines(value: i64) -> i64 {
    value + 1
}

pub fn inv_tax(value: i64) -> i64 {
    value + 2
}

pub fn inv_footer(value: i64) -> i64 {
    value + 3
}

pub fn exp_open(value: i64) -> i64 {
    value + 10
}

pub fn exp_row(value: i64) -> i64 {
    value + 11
}

pub fn exp_escape(value: i64) -> i64 {
    value + 12
}

pub fn exp_close(value: i64) -> i64 {
    value + 13
}

pub fn led_open(value: i64) -> i64 {
    value + 20
}

pub fn led_post(value: i64) -> i64 {
    value + 21
}

pub fn led_balance(value: i64) -> i64 {
    value + 22
}

pub fn led_close(value: i64) -> i64 {
    value + 23
}

pub fn stat_mean(value: i64) -> i64 {
    value + 30
}

pub fn stat_spread(value: i64) -> i64 {
    value + 31
}

pub fn stat_low(value: i64) -> i64 {
    value + 32
}

pub fn stat_high(value: i64) -> i64 {
    value + 33
}

pub fn rk_score(value: i64) -> i64 {
    value + 40
}

pub fn rk_sort(value: i64) -> i64 {
    value + 41
}

pub fn rk_trim(value: i64) -> i64 {
    value + 42
}

pub fn rk_label(value: i64) -> i64 {
    value + 43
}

pub fn c_fetch(value: i64) -> i64 {
    value + 50
}

pub fn c_diff(value: i64) -> i64 {
    value + 51
}

pub fn c_apply(value: i64) -> i64 {
    value + 52
}

pub fn c_flush(value: i64) -> i64 {
    value + 53
}

pub fn c_log(value: i64) -> i64 {
    value + 54
}

pub fn v_trim(value: i64) -> i64 {
    value + 60
}

pub fn v_check(value: i64) -> i64 {
    value + 61
}

pub fn v_coerce(value: i64) -> i64 {
    value + 62
}

pub fn v_report(value: i64) -> i64 {
    value + 63
}

pub fn m_left(value: i64) -> i64 {
    value + 70
}

pub fn m_right(value: i64) -> i64 {
    value + 71
}

pub fn m_join(value: i64) -> i64 {
    value + 72
}

pub fn m_tag(value: i64) -> i64 {
    value + 73
}

pub fn r_seed(value: i64) -> i64 {
    value + 80
}

pub fn r_step(value: i64) -> i64 {
    value + 81
}

pub fn r_mix(value: i64) -> i64 {
    value + 82
}

pub fn r_end(value: i64) -> i64 {
    value + 83
}
