fn a_head(value: i64) -> i64 {
    value * 2
}

fn a_body(value: i64) -> i64 {
    value * 3
}

fn a_tail(value: i64) -> i64 {
    value * 4
}

fn a_mark(value: i64) -> i64 {
    value * 5
}

fn b_split(value: i64) -> i64 {
    value * 3
}

fn b_strip(value: i64) -> i64 {
    value * 4
}

fn b_key(value: i64) -> i64 {
    value * 5
}

fn b_value(value: i64) -> i64 {
    value * 6
}

fn q_wrap(value: i64) -> i64 {
    value * 4
}

fn q_weigh(value: i64) -> i64 {
    value * 5
}

fn q_seal(value: i64) -> i64 {
    value * 6
}

fn q_ship(value: i64) -> i64 {
    value * 7
}

pub fn sum_order(order: i64) -> i64 {
    let head = a_head(order);
    let body = a_body(head);
    let tail = a_tail(body);
    a_mark(tail)
}

pub fn parse_line(line: i64) -> i64 {
    let parts = b_split(line);
    let clean = b_strip(parts);
    b_key(clean) + b_value(clean)
}

pub fn pack_items(items: &[i64]) -> i64 {
    let mut packed = 0;
    for item in items {
        packed += q_seal(q_weigh(q_wrap(*item)));
    }
    q_ship(packed)
}
