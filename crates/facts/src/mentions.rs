use kernel::Span;

/// Return the 1-based lines, each once and in order, where `name` starts inside one of the
/// `errors` byte spans; the bytes compare with ASCII case folded, and matches do not overlap.
#[must_use]
pub fn unparsed_mentions(source: &str, errors: &[Span], name: &str) -> Vec<u32> {
    let haystack = source.as_bytes();
    let needle = name.as_bytes();
    let mut lines: Vec<u32> = Vec::new();
    let mut newlines = Newlines::default();
    let mut at = 0;
    while at <= haystack.len() {
        let matched = haystack
            .get(at..at + needle.len())
            .is_some_and(|window| window.eq_ignore_ascii_case(needle));
        if !matched {
            at += 1;
            continue;
        }
        if inside(errors, at) {
            let line = newlines.before(haystack, at) + 1;
            if !lines.contains(&line) {
                lines.push(line);
            }
        }
        // An empty name matches at every position, so the scan still moves on.
        at += needle.len().max(1);
    }
    lines
}

fn inside(errors: &[Span], at: usize) -> bool {
    let at = u32::try_from(at).unwrap_or(u32::MAX);
    errors
        .iter()
        .any(|span| span.start() <= at && at < span.end())
}

// A running count of line feeds, for positions that only grow.
#[derive(Default)]
struct Newlines {
    counted_to: usize,
    count: u32,
}

impl Newlines {
    fn before(&mut self, haystack: &[u8], at: usize) -> u32 {
        let fed = haystack[self.counted_to..at]
            .split(|&byte| byte == b'\n')
            .count()
            - 1;
        self.count = self
            .count
            .saturating_add(u32::try_from(fed).unwrap_or(u32::MAX));
        self.counted_to = at;
        self.count
    }
}
