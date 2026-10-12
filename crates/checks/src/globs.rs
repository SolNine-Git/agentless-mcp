//! The boundary glob dialect: `*` inside one segment, `**` over whole segments, no backtracking.

const GLOBSTAR: &str = "**";

pub(crate) fn in_boundary(path: &str, globs: &[String]) -> bool {
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
