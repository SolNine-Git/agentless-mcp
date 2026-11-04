//! The self-glob negative: the test module's `use super::*` names this file, so a call of `drop`
//! bound every `Drop::drop` method of the file.

pub struct Head;
pub struct Tail;
pub struct Slot;

impl Drop for Head {
    fn drop(&mut self) {}
}

impl Drop for Tail {
    fn drop(&mut self) {}
}

impl Drop for Slot {
    fn drop(&mut self) {}
}

pub fn release(tail: Tail) {
    drop(tail);
}

pub fn send(slot: Slot) {
    drop(slot);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn releases_the_tail() {
        release(Tail);
    }
}
