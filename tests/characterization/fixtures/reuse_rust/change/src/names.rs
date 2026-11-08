pub fn clean_label(data: &[u8]) -> String {
    let mut label = String::new();
    for word in String::from_utf8_lossy(data).split_whitespace() {
        if !label.is_empty() {
            label.push(' ');
        }
        label.push_str(&capitalized(word));
    }
    label
}
