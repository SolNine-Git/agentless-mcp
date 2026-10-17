//! The operating system calls that std offers no safe form of: a close that reports its error, and
//! the start of a test command as the reference starts one, in a new session with no inherited
//! descriptor above 2 and with the reference's exec error. Every unsafe block of io is here.

use std::ffi::{CString, OsString, c_char, c_int};
use std::fs::File;
use std::io;
use std::os::fd::IntoRawFd;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::process::CommandExt;
use std::process::Command;

use rustix::process::{Resource, getrlimit};

// The search list when the environment holds no PATH, and the most descriptors the loop marks.
const DEFAULT_PATH: &[u8] = b"/bin:/usr/bin";
const MAX_DESCRIPTOR_BOUND: u64 = 65_536;

// The drop of a File discards the close's error, which a deferred write failure (NFS, a disk
// quota) can give after a flush that succeeded.
pub(crate) fn close_checked(file: File) -> io::Result<()> {
    let descriptor = file.into_raw_fd();
    // SAFETY: into_raw_fd gave up the File's ownership, so this is the descriptor's only close.
    if unsafe { libc::close(descriptor) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

// The exec of one test command, built before the fork: the child allocates nothing.
pub(crate) struct Exec {
    candidates: Vec<CString>,
    // The C strings that the two pointer tables point into; they move with the tables.
    _argv: Vec<CString>,
    _envp: Vec<CString>,
    // Null-terminated tables of the strings' exposed addresses, in the layout execve reads.
    argv_table: Vec<usize>,
    envp_table: Vec<usize>,
    descriptor_bound: c_int,
}

impl Exec {
    // `None` for a word with a NUL byte: std then refuses the spawn before it forks.
    pub(crate) fn prepare(
        program: &str,
        args: &[String],
        env: &[(String, OsString)],
    ) -> Option<Self> {
        let words: Vec<CString> = std::iter::once(program)
            .chain(args.iter().map(String::as_str))
            .map(|word| CString::new(word).ok())
            .collect::<Option<_>>()?;
        let envp: Vec<CString> = env
            .iter()
            .map(|(name, value)| {
                let mut entry = name.as_bytes().to_vec();
                entry.push(b'=');
                entry.extend_from_slice(value.as_bytes());
                CString::new(entry).ok()
            })
            .collect::<Option<_>>()?;
        let path = env.iter().find(|(name, _)| name == "PATH");
        let search = path.map_or(DEFAULT_PATH, |(_, value)| value.as_bytes());
        Some(Self {
            candidates: candidates(program.as_bytes(), search)?,
            argv_table: table(&words),
            envp_table: table(&envp),
            _argv: words,
            _envp: envp,
            descriptor_bound: descriptor_bound(),
        })
    }

    // Runs in the child between the fork and the exec: async-signal-safe calls only.
    fn run(&self) -> io::Result<()> {
        rustix::process::setsid()
            .map_err(|errno| io::Error::from_raw_os_error(errno.raw_os_error()))?;
        mark_cloexec_above_stderr(self.descriptor_bound)?;
        Err(exec_first(self))
    }
}

// A program with a slash is run as named; any other name is tried in each search entry in turn,
// an empty entry meaning the working directory.
fn candidates(program: &[u8], search: &[u8]) -> Option<Vec<CString>> {
    if program.contains(&b'/') {
        return CString::new(program).ok().map(|named| vec![named]);
    }
    search
        .split(|&byte| byte == b':')
        .map(|entry| {
            let mut joined = entry.to_vec();
            if !entry.is_empty() && !entry.ends_with(b"/") {
                joined.push(b'/');
            }
            joined.extend_from_slice(program);
            CString::new(joined).ok()
        })
        .collect()
}

fn table(strings: &[CString]) -> Vec<usize> {
    let addresses = strings.iter().map(|text| text.as_ptr().expose_provenance());
    addresses.chain([0]).collect()
}

// The soft limit on descriptors bounds the loop that marks them where close_range is missing.
fn descriptor_bound() -> c_int {
    let soft = getrlimit(Resource::Nofile).current;
    let bound = soft
        .unwrap_or(MAX_DESCRIPTOR_BOUND)
        .min(MAX_DESCRIPTOR_BOUND);
    c_int::try_from(bound).unwrap_or(c_int::MAX)
}

// The start the reference gives a test command: setsid in the child, every descriptor above 2
// closed at the exec, and an exec of the candidates with no shell run for a file with no #! line.
pub(crate) fn start_in_new_session(command: &mut Command, exec: Exec) {
    // SAFETY: between fork and exec the closure makes only async-signal-safe calls and never
    // allocates: `exec` holds every string and table, built before the spawn.
    unsafe {
        command.pre_exec(move || exec.run());
    }
}

// The descriptors above 2 become close-on-exec, so std's own error pipe still works until the exec.
fn mark_cloexec_above_stderr(bound: c_int) -> io::Result<()> {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    {
        let flags = libc::CLOSE_RANGE_CLOEXEC.cast_signed();
        // SAFETY: close_range only sets a flag on this process's descriptors; it reads no memory.
        if unsafe { libc::close_range(3, libc::c_uint::MAX, flags) } == 0 {
            return Ok(());
        }
    }
    for descriptor in 3..bound {
        // SAFETY: F_GETFD reads no memory, and a number with no open descriptor gives EBADF.
        let flags = unsafe { libc::fcntl(descriptor, libc::F_GETFD) };
        if flags < 0 {
            continue;
        }
        // SAFETY: F_SETFD on the open descriptor just read only sets its flags; it reads no memory.
        if unsafe { libc::fcntl(descriptor, libc::F_SETFD, flags | libc::FD_CLOEXEC) } < 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

// Each candidate in turn; the first error that is not a missing name or a missing directory is the
// answer, else the last error.
fn exec_first(exec: &Exec) -> io::Error {
    let argv = exec.argv_table.as_ptr().cast::<*const c_char>();
    let envp = exec.envp_table.as_ptr().cast::<*const c_char>();
    let mut first = None;
    let mut last = libc::ENOENT;
    for candidate in &exec.candidates {
        // SAFETY: each string ends in NUL, and each table is a null-terminated array of the
        // addresses of NUL-terminated strings that `exec` owns.
        unsafe { libc::execve(candidate.as_ptr(), argv, envp) };
        let code = io::Error::last_os_error()
            .raw_os_error()
            .unwrap_or(libc::EINVAL);
        if first.is_none() && code != libc::ENOENT && code != libc::ENOTDIR {
            first = Some(code);
        }
        last = code;
    }
    io::Error::from_raw_os_error(first.unwrap_or(last))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shown(found: Option<Vec<CString>>) -> Vec<String> {
        let found = found.expect("no NUL byte");
        let text = |name: &CString| String::from_utf8_lossy(name.as_bytes()).into_owned();
        found.iter().map(text).collect()
    }

    #[test]
    fn a_name_with_a_slash_is_run_as_named_and_any_other_is_searched() {
        assert_eq!(shown(candidates(b"./run.sh", b"/bin")), ["./run.sh"]);
        assert_eq!(
            shown(candidates(b"make", b"/usr/bin::/opt/")),
            ["/usr/bin/make", "make", "/opt/make"]
        );
        assert_eq!(shown(candidates(b"", b"/bin")), ["/bin/"]);
        assert!(candidates(b"a\0b", b"/bin").is_none());
    }

    #[test]
    fn a_word_with_a_nul_byte_leaves_the_refusal_to_std() {
        let env = [("PATH".to_owned(), OsString::from("/bin"))];
        assert!(Exec::prepare("sh", &["a\0b".to_owned()], &env).is_none());
        let env = [("X".to_owned(), OsString::from("a\0b"))];
        assert!(Exec::prepare("sh", &[], &env).is_none());
    }

    #[test]
    fn each_table_ends_in_a_null_entry_after_its_strings() {
        let env = [("PATH".to_owned(), OsString::from("/bin"))];
        let exec = Exec::prepare("sh", &["-c".to_owned()], &env).expect("no NUL byte");
        assert_eq!(exec.argv_table.len(), 3);
        assert_eq!(exec.argv_table.last(), Some(&0));
        assert_eq!(exec.envp_table.len(), 2);
        assert!(exec.envp_table[0] != 0 && exec.envp_table[1] == 0);
        assert!(exec.descriptor_bound > 2);
    }
}
