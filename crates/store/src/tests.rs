mod cases;
mod discards;
mod lookups;
mod opens;
mod parity;
mod texts;
mod values;

use crate::TagsDb;

// The hook runs a lint on a worker thread and stops waiting at its deadline.
#[test]
fn an_open_database_moves_between_threads() {
    fn send<T: Send>() {}
    send::<TagsDb>();
}
