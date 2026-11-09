pub fn decode_label(raw: &[u8]) -> String {
    let text = String::from_utf8_lossy(raw);
    let words: Vec<String> = text.split_whitespace().map(|word| capitalized(word)).collect();
    words.join(" ")
}
