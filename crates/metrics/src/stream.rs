use kernel::Key;
use syntax::{NodeId, Parsed};

use crate::digest::Joined;

pub(crate) fn body_key(parsed: &Parsed, unit: NodeId, omit: NodeId) -> Key {
    let mut joined = Joined::new(" ");
    for token in syntax::tokens(parsed, unit, Some(omit)) {
        joined.piece(&[&token]);
    }
    joined.key()
}
