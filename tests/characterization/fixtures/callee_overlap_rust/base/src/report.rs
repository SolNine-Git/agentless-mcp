pub struct Report;

fn header(value: i64) -> i64 {
    value * 7
}

fn body(value: i64) -> i64 {
    value * 8
}

fn footer(value: i64) -> i64 {
    value * 9
}

fn totals(value: i64) -> i64 {
    value * 10
}

pub fn render_report(seed: i64) -> i64 {
    let top = header(seed);
    let middle = body(top);
    totals(footer(middle))
}
