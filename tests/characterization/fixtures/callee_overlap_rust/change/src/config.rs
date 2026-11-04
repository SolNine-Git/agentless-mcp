fn read_text(value: i64) -> i64 {
    value * 5
}

fn parse_toml(value: i64) -> i64 {
    value * 6
}

fn check_config(value: i64) -> i64 {
    value * 7
}

fn check_manifest(value: i64) -> i64 {
    value * 8
}

pub fn load_config(path: i64) -> i64 {
    let text = read_text(path);
    check_config(parse_toml(text))
}

pub fn load_manifest(path: i64) -> i64 {
    let text = read_text(path);
    check_manifest(parse_toml(text))
}
