"""Exclusive-or-refuse file locking, on whichever platform this is running on.

The index writer needs one property and only one: a second concurrent run must
be *told* that the lock is held rather than queue behind the first. Both
implementations below deliver exactly that, and both are non-blocking, so
neither can turn a busy cache into a hang.

**POSIX** uses ``fcntl.flock``, which locks the whole file and releases on
close as well as on request.

**Windows** uses ``msvcrt.locking`` with ``LK_NBLCK``, which locks a byte range
rather than a file, so the range is fixed at the first byte and the release
seeks back to it. This path is best effort and untested on Windows hardware --
see the Windows section of the README for exactly what that covers.

The two platform modules are resolved with :func:`importlib.import_module`
rather than imported at the top of this file because neither exists on the
other platform: ``import fcntl`` is what makes a POSIX-only package, which is
the thing being fixed here. Nothing about this is a dependency cycle.

Which implementation runs is decided by :func:`agentless_mcp.util.platforms.family`,
so the dispatch is unit-testable on one platform without pretending to be the
other. What cannot be tested off Windows is the Windows locking call itself.
"""

import importlib
import logging
from collections.abc import Iterator
from contextlib import contextmanager
from pathlib import Path
from typing import IO

from agentless_mcp.util.platforms import WINDOWS

logger = logging.getLogger(__name__)

# msvcrt locks a byte range, so the whole protocol is "the first byte of the
# file is the lock". Its length is not a tuning knob: both sides must name the
# same range, and one byte is the smallest range that exists.
_WINDOWS_LOCK_BYTES = 1


class LockUnavailableError(Exception):
    """Raised when the lock could not be taken, held elsewhere or unsupported.

    Deliberately not one of the package's domain errors: this module is a
    stdlib-leaf utility, and the caller that knows *what* was locked is the
    one that can say so in a message a user can act on.
    """


@contextmanager
def exclusive(path: Path, *, flavour: str) -> Iterator[None]:
    """Hold an exclusive lock on ``path``, or raise :class:`LockUnavailableError`.

    The lock file is created if it is missing and is never removed: unlinking
    it would let a second process create and lock a *different* file with the
    same name while the first still holds the old one. Append rather than
    write for the same reason: truncating happens before the lock is taken, so
    it is the one write to this file no mutual exclusion covers.
    """
    handle = path.open("a", encoding="utf-8")
    try:
        _acquire(handle, flavour)
        try:
            yield
        finally:
            _release(handle, flavour)
    finally:
        handle.close()


def _acquire(handle: IO[str], flavour: str) -> None:
    """Take the lock without waiting for it."""
    if flavour == WINDOWS:
        msvcrt = importlib.import_module("msvcrt")
        handle.seek(0)
        try:
            msvcrt.locking(handle.fileno(), msvcrt.LK_NBLCK, _WINDOWS_LOCK_BYTES)
        except OSError as exc:
            raise LockUnavailableError from exc
        return

    fcntl = importlib.import_module("fcntl")
    try:
        fcntl.flock(handle.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
    except OSError as exc:
        # `BlockingIOError` is the lock-is-held answer; `ENOLCK` and
        # `EOPNOTSUPP` from an NFS, FUSE or overlay mount are the this-mount-
        # cannot-lock answer. Both leave the caller without exclusion, so both
        # are the refusal rather than a raw errno escaping this leaf.
        raise LockUnavailableError from exc


def _release(handle: IO[str], flavour: str) -> None:
    """Release the lock, tolerating a handle the OS already dropped it from."""
    if flavour == WINDOWS:
        msvcrt = importlib.import_module("msvcrt")
        handle.seek(0)
        try:
            msvcrt.locking(handle.fileno(), msvcrt.LK_UNLCK, _WINDOWS_LOCK_BYTES)
        except OSError as exc:
            # Closing the handle drops the lock anyway, and raising from here
            # would replace whatever the caller's body was already failing
            # with. Logged rather than swallowed: it should never happen.
            logger.warning("releasing the lock on %s failed: %s", handle.name, exc)
        return

    fcntl = importlib.import_module("fcntl")
    try:
        fcntl.flock(handle.fileno(), fcntl.LOCK_UN)
    except OSError as exc:
        # The Windows branch above states why a release never raises, and the
        # reasoning is the platform's, not the flavour's: `handle.close()` in
        # :func:`exclusive` drops the lock whatever this call did, so raising
        # here would only replace the caller's own failure with one about the
        # release. It would also arrive as a raw `OSError` rather than a
        # `LockUnavailableError`, so the handler that catches this module's
        # refusal would miss it.
        logger.warning("releasing the lock on %s failed: %s", handle.name, exc)
