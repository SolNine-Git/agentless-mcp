use kernel::Key;
use sha2::{Digest, Sha256};

pub(crate) struct Joined {
    hasher: Sha256,
    separator: &'static str,
    started: bool,
}

impl Joined {
    pub(crate) fn new(separator: &'static str) -> Self {
        Self {
            hasher: Sha256::new(),
            separator,
            started: false,
        }
    }

    // One piece of the joined text is the concatenation of `parts`.
    pub(crate) fn piece(&mut self, parts: &[&str]) {
        if self.started {
            self.hasher.update(self.separator);
        }
        self.started = true;
        for part in parts {
            self.hasher.update(part);
        }
    }

    pub(crate) fn key(self) -> Key {
        Key::new(self.hasher.finalize().into())
    }
}
