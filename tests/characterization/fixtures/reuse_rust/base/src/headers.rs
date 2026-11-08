pub fn parse_header(line: &str) -> Option<(String, String)> {
    let (name, value) = line.split_once(':')?;
    let key = name.trim().to_ascii_lowercase();
    Some((key, value.trim_start().to_owned()))
}
