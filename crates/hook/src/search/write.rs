//! The shell writes: the command forms that name a file write, and the source paths a command
//! names. A program that only might write, such as `uv run` or `make`, is left to the Stop scan.

use std::sync::LazyLock;

use regex::Regex;

// A write has many spellings. The redirect form is `redirects`, which needs a lookahead.
const WRITE_FORMS: &str = concat!(
    r"\btee\b",
    r"|\bsed\b[^|;]*?\s-{1,2}i",
    r"|\bperl\b\s+-\w*i",
    r"|\bwrite_text\b|\bwrite_bytes\b|\bwritelines\b|\.write\(",
    r#"|\bopen\s*\([^)]*['"][wax]"#,
    r"|\bpatch\b|\bgit\s+apply\b",
    r"|\b(?:cp|mv|rsync|install|truncate|dd)\s",
    r"|--write\b|--in-place\b",
);
const PATH_TOKEN: &str = r"[\w./~+-]+\.\w+";
const DEV_NULL: &str = "/dev/null";

static WRITES: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(WRITE_FORMS).expect("the write forms compile"));
static PATHS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(PATH_TOKEN).expect("the path token compiles"));

// True when the command holds a form that writes a file.
pub(crate) fn writes(command: &str) -> bool {
    redirects(command) || WRITES.is_match(command)
}

// The tier-1 source paths that the command names, in order.
pub(crate) fn source_paths(command: &str) -> Vec<&str> {
    (PATHS.find_iter(command))
        .map(|found| found.as_str())
        .filter(|token| checks::pair_language(token).is_some())
        .collect()
}

// A `>` or `>>` whose target is no `&` descriptor and no /dev/null. Both lengths are tried, as a
// backtracking match would.
fn redirects(command: &str) -> bool {
    command.match_indices('>').any(|(at, _)| {
        let single = &command[at + 1..];
        let double = single.strip_prefix('>');
        [Some(single), double].into_iter().flatten().any(has_target)
    })
}

fn has_target(rest: &str) -> bool {
    let rest = rest.trim_start();
    rest.chars().next().is_some_and(|first| first != '&') && !rest.starts_with(DEV_NULL)
}

#[cfg(test)]
mod tests;
