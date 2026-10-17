mod blobs;
mod cases;
mod discards;
mod indexed;
mod lookups;
mod opens;
mod parity;
mod plans;
mod references;
mod sources;
mod sweeps;
mod texts;
mod values;
mod writes;

use crate::TagsDb;

// The hook runs a lint on a worker thread and stops waiting at its deadline.
#[test]
fn an_open_database_moves_between_threads() {
    fn send<T: Send>() {}
    send::<TagsDb>();
}
