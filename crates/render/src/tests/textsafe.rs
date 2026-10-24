use std::borrow::Cow;
use std::collections::HashMap;

use super::rows::cases;
use crate::one_line;

#[test]
fn every_code_point_escapes_as_the_table_says() {
    let escapes: HashMap<u32, String> = cases::<Vec<(u32, String)>>("textsafe", "escapes")
        .into_iter()
        .collect();
    for c in (0..=0x10_ffff).filter_map(char::from_u32) {
        let text = c.to_string();
        let code = u32::from(c);
        let expected = escapes.get(&code).map_or(text.as_str(), String::as_str);
        assert_eq!(one_line(&text), expected, "U+{code:04X}");
    }
}

#[test]
fn a_whole_text_escapes_as_the_table_says() {
    let texts: Vec<(String, String)> = cases("textsafe", "texts");
    for (text, expected) in texts {
        assert_eq!(one_line(&text), expected, "{text:?}");
    }
}

#[test]
fn a_safe_text_is_returned_without_a_copy() {
    assert!(matches!(one_line("tab\tkept path/a.py"), Cow::Borrowed(_)));
}
