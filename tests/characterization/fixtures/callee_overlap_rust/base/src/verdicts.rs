//! The parameter shadow negative: the parameter `record` of `integer` spells the free function
//! `record`, which neither function calls.

pub struct Pairs {
    pub values: Vec<i64>,
}

pub fn record(line: &str) -> Pairs {
    Pairs {
        values: vec![line.len() as i64],
    }
}

fn lookup(record: &Pairs, name: &str) -> Option<i64> {
    record.values.get(name.len()).copied()
}

fn absent(what: &str, name: &str) -> String {
    format!("{what} {name} is absent")
}

pub fn integer(record: &Pairs, name: &str) -> Result<i64, String> {
    match lookup(record, name) {
        Some(value) => Ok(value),
        None => Err(absent("an integer", name)),
    }
}
