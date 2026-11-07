//! The boundary glob dialect: `*` inside one segment, `**` over whole segments, no backtracking,
//! and the `{a,b}` groups that expand to one glob per choice before a match.

const GLOBSTAR: &str = "**";

/// The most globs that one brace expansion may give.
pub const MAX_BRACE_GLOBS: usize = 256;

/// Returns true when one of `globs` matches `path` in the boundary glob dialect.
#[must_use]
pub fn in_boundary(path: &str, globs: &[String]) -> bool {
    let segments: Vec<&str> = path.split('/').collect();
    globs.iter().any(|glob| {
        let elements: Vec<&str> = glob.split('/').collect();
        glob_matches(&elements, &segments)
    })
}

// reached[j]: the elements so far consume exactly segments[..j]. One pass per element,
// so the cost grows with glob length times path length and nothing backtracks.
fn glob_matches(elements: &[&str], segments: &[&str]) -> bool {
    let mut reached = vec![false; segments.len() + 1];
    reached[0] = true;
    let last = elements.len().saturating_sub(1);
    for (index, element) in elements.iter().enumerate() {
        reached = if *element == GLOBSTAR {
            globstar(&reached, segments, index == last)
        } else {
            step(&reached, segments, element)
        };
    }
    reached[segments.len()]
}

fn step(reached: &[bool], segments: &[&str], element: &str) -> Vec<bool> {
    let mut next = vec![false];
    next.extend(
        reached
            .iter()
            .zip(segments)
            .map(|(was, segment)| *was && segment_matches(element, segment)),
    );
    next
}

// ** consumes whole non-empty segments: one or more at the end of a glob, any number inside.
fn globstar(reached: &[bool], segments: &[&str], last: bool) -> Vec<bool> {
    let mut spread = vec![false];
    for (j, segment) in segments.iter().enumerate() {
        let more = !segment.is_empty() && (reached[j] || spread[j]);
        spread.push(more);
    }
    if last {
        return spread;
    }
    reached
        .iter()
        .zip(&spread)
        .map(|(was, more)| *was || *more)
        .collect()
}

// A star stays inside one segment, so each piece between stars takes its leftmost
// place after the previous one, and no choice is ever undone.
fn segment_matches(pattern: &str, text: &str) -> bool {
    let pieces: Vec<&str> = pattern.split('*').collect();
    let [head, middle @ .., tail] = pieces.as_slice() else {
        return pattern == text;
    };
    let Some(end) = text.len().checked_sub(tail.len()) else {
        return false;
    };
    if end < head.len() || !text.starts_with(head) || !text.ends_with(tail) {
        return false;
    }
    let mut position = head.len();
    for piece in middle {
        match text[position..end].find(piece) {
            Some(found) => position += found + piece.len(),
            None => return false,
        }
    }
    true
}

/// Returns the globs that `glob` stands for when each `{a,b}` group is one glob per choice, in
/// order, or `None` when the expansion gives more than [`MAX_BRACE_GLOBS`] globs.
#[must_use]
pub fn expand_braces(glob: &str) -> Option<Vec<String>> {
    let mut expanded: Vec<String> = Vec::new();
    let mut pending = vec![glob.to_owned()];
    while let Some(next) = pending.pop() {
        match first_group(&next) {
            Some(group) => pending.extend(group.choices(&next).into_iter().rev()),
            None if expanded.contains(&next) => {}
            None => expanded.push(next),
        }
        if expanded.len() + pending.len() > MAX_BRACE_GLOBS {
            return None;
        }
    }
    Some(expanded)
}

// A brace pair at byte offsets `open` and `close` and the offsets of its top-level commas.
struct Group {
    open: usize,
    close: usize,
    commas: Vec<usize>,
}

impl Group {
    fn choices(&self, glob: &str) -> Vec<String> {
        let (head, tail) = (&glob[..self.open], &glob[self.close + 1..]);
        let starts = std::iter::once(self.open + 1).chain(self.commas.iter().map(|at| at + 1));
        let ends = self
            .commas
            .iter()
            .copied()
            .chain(std::iter::once(self.close));
        starts
            .zip(ends)
            .map(|(start, end)| format!("{head}{}{tail}", &glob[start..end]))
            .collect()
    }
}

// The first brace pair that holds a top-level comma. A pair with no comma and an unclosed brace
// are literal text, as a shell reads them.
fn first_group(glob: &str) -> Option<Group> {
    (glob.match_indices('{')).find_map(|(open, _)| group_at(glob, open))
}

fn group_at(glob: &str, open: usize) -> Option<Group> {
    let mut depth = 0_usize;
    let mut commas = Vec::new();
    for (offset, byte) in glob.bytes().enumerate().skip(open + 1) {
        match byte {
            b'{' => depth += 1,
            b'}' if depth == 0 => {
                return (!commas.is_empty()).then_some(Group {
                    open,
                    close: offset,
                    commas,
                });
            }
            b'}' => depth -= 1,
            b',' if depth == 0 => commas.push(offset),
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::{MAX_BRACE_GLOBS, expand_braces, in_boundary};

    fn expanded(glob: &str) -> Vec<String> {
        expand_braces(glob).unwrap_or_else(|| panic!("{glob} expands"))
    }

    #[test]
    fn each_choice_of_a_group_is_one_glob_in_order() {
        assert_eq!(expanded("src/{a,b}/**"), ["src/a/**", "src/b/**"]);
        assert_eq!(expanded("*.{rs,toml,}"), ["*.rs", "*.toml", "*."]);
    }

    #[test]
    fn groups_expand_left_to_right_and_nested_groups_expand_inside_their_choice() {
        assert_eq!(expanded("{a,b}{1,2}"), ["a1", "a2", "b1", "b2"]);
        assert_eq!(expanded("{x,{y,z}}.py"), ["x.py", "y.py", "z.py"]);
    }

    #[test]
    fn a_pair_with_no_comma_and_an_unclosed_brace_stay_literal() {
        assert_eq!(expanded("{a}.rs"), ["{a}.rs"]);
        assert_eq!(expanded("{a,b"), ["{a,b"]);
        assert_eq!(expanded("{{a,b}}"), ["{a}", "{b}"]);
    }

    #[test]
    fn a_repeated_choice_gives_one_glob() {
        assert_eq!(expanded("{a,a,b}"), ["a", "b"]);
    }

    #[test]
    fn an_expansion_past_the_bound_gives_none() {
        assert_eq!(expand_braces(&"{a,b}".repeat(9)), None);
        let widest = expand_braces(&"{a,b}".repeat(8)).map(|globs| globs.len());
        assert_eq!(widest, Some(MAX_BRACE_GLOBS));
    }

    #[test]
    fn an_expanded_glob_matches_in_the_boundary_dialect() {
        let globs = expanded("crates/{hook,app}/src/*.rs");
        assert!(in_boundary("crates/app/src/lib.rs", &globs));
        assert!(!in_boundary("crates/io/src/lib.rs", &globs));
    }
}
