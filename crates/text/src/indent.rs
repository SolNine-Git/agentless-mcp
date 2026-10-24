//! The common indentation of the lines of a text.

/// Empties lines of only spaces and tabs in `text` and removes the margin the other lines share.
#[must_use]
pub fn dedent(text: &str) -> String {
    let lines: Vec<&str> = text.split('\n').map(blanked).collect();
    let margin = lines
        .iter()
        .filter(|line| !line.is_empty())
        .map(|line| indent(line))
        .reduce(common_prefix)
        .unwrap_or_default();
    let dedented: Vec<&str> = lines
        .iter()
        .map(|line| line.strip_prefix(margin).unwrap_or(line))
        .collect();
    dedented.join("\n")
}

fn is_margin(c: char) -> bool {
    matches!(c, ' ' | '\t')
}

fn blanked(line: &str) -> &str {
    if line.chars().all(is_margin) {
        ""
    } else {
        line
    }
}

fn indent(line: &str) -> &str {
    let text_start = line.len() - line.trim_start_matches(is_margin).len();
    &line[..text_start]
}

// Margins hold only ASCII spaces and tabs, so a byte count is a char boundary.
fn common_prefix<'a>(left: &'a str, right: &str) -> &'a str {
    let shared = left
        .bytes()
        .zip(right.bytes())
        .take_while(|(x, y)| x == y)
        .count();
    &left[..shared]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::check_pairs;

    #[test]
    fn dedent_cases() {
        check_pairs(env!("CARGO_MANIFEST_DIR"), "text", "dedent", dedent);
    }
}
