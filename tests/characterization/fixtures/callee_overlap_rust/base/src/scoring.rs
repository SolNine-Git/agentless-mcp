fn s_base(value: i64) -> i64 {
    value * 8
}

fn s_bonus(value: i64) -> i64 {
    value * 9
}

fn s_cap(value: i64) -> i64 {
    value * 10
}

fn s_round(value: i64) -> i64 {
    value * 11
}

fn p_left(value: i64) -> i64 {
    value * 12
}

fn p_right(value: i64) -> i64 {
    value * 13
}

pub fn score(points: i64) -> i64 {
    let raw = s_base(points);
    s_round(s_cap(s_bonus(raw)))
}

pub fn pair_up(value: i64) -> i64 {
    p_left(value) + p_right(value)
}
