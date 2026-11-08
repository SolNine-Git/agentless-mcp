//! The retired hooks: gate-check and gate-mark. A settings file of an earlier release can still
//! run them, so each reads its payload and exits 0 with no output until hook install removes it.

use std::io::Read;

/// Reads the payload on `stdin` and returns 0. It writes nothing.
pub fn retired(stdin: &mut dyn Read) -> u8 {
    // Claude Code writes the whole payload; a hook that exits first can break that write. A
    // failed read leaves nothing to do.
    drop(std::io::copy(stdin, &mut std::io::sink()));
    0
}
