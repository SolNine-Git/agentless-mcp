pub fn trim_dot(value: &str) -> &str {
    value.trim().trim_end_matches('.')
}
