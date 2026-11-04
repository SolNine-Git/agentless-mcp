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

impl std::fmt::Display for Report {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let text = totals(footer(body(header(0))));
        write!(out, "{text}")
    }
}

#[cfg(test)]
mod tests {
    use super::{body, footer, header, totals};

    #[test]
    fn renders_a_report() {
        assert_eq!(totals(footer(body(header(1)))), 7);
    }
}
