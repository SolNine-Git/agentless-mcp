pub fn split_field(text: &str) -> Option<(String, String)> {
    let (label, rest) = text.split_once(':')?;
    let field = label.trim().to_ascii_lowercase();
    Some((field, rest.trim_start().to_owned()))
}
