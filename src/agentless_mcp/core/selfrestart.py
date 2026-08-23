"""Restarting a long-running server when its own install changes.

A long-running HTTP process never re-imports code: after a ``pip`` or ``uv``
upgrade, it keeps serving the modules it loaded at startup while the version
handshake reports that loaded -- now stale -- version as if it were current.
Reconnecting clients does nothing, because reconnect refreshes the
connection, not the process. The stdio transport is immune by construction
(one process per client connection), which is exactly what makes the HTTP
drift easy to miss.

OS-packaged daemons get restarted by their package manager's postinst hook;
pip and uv have no postinst, so no package-manager restart path exists and
the process has to own the problem. The design here mirrors the background
grammar warm and the background index refresh: notice, heal, stay out of the
way, and take an opt-out.

**Detection** is a slow poll of the install fingerprint -- the distribution
version plus a digest of its dist-info ``RECORD`` -- rather than filesystem
watching, because a poll works identically on every platform PyPI serves.
An editable install changes its ``RECORD`` on reinstall and version bump but
never on source edits, so a development server does not bounce on every file
save; only install events count. A fingerprint that reads as *absent*
mid-poll is an upgrade in progress (``RECORD`` is deleted before it is
rewritten), so absence never triggers -- only a fingerprint that is present
and different does.

**Restart** goes through the server's own front door: the monitor raises
``SIGINT`` in the process, which is the graceful-shutdown path the HTTP
stack already implements -- stop accepting, drain in-flight requests,
return from ``run()``. Only then does the process replace itself with
``os.execv`` of its original argv: same pid, same flags, new code, no
supervisor required. All of this server's state is derived and on disk (the
tag caches, the grammar caches), which is what makes exec-in-place safe
here. On Windows, where exec with open sockets is unreliable, the process
instead exits 0 after the same graceful shutdown and a supervisor's
``Restart=`` completes the loop.
"""

import hashlib
import logging
import os
import signal
import sys
import threading
import time
from dataclasses import dataclass
from importlib.metadata import PackageNotFoundError, distribution

from agentless_mcp.core import grammars
from agentless_mcp.util import platforms

logger = logging.getLogger(__name__)

ENV_NO_AUTO_RESTART = "AGENTLESS_MCP_NO_AUTO_RESTART"

# Slow on purpose: an upgrade is a human-timescale event, and the only cost
# of a late restart is a few more answers from the old code. Two reads of a
# metadata file every interval is noise next to one tool call.
POLL_SECONDS = 30.0

# How long this process must have been up before it will restart itself. A
# change seen sooner is held, not discarded: the restart happens once this has
# passed, so no upgrade is ever lost, and the rate at which a process can bounce
# is bounded rather than being one restart per poll.
#
# This is a rate bound, not a loop breaker. An install whose fingerprint is
# genuinely unstable -- a backend that regenerates RECORD non-deterministically,
# an external sync touching dist-info -- would still restart once per interval
# here, because a process that has replaced its own image cannot remember how
# many times it has done so. Breaking that properly needs a counter carried
# across the exec; it is not built until such an install is actually observed,
# and the held-restart log line above is what would make it visible.
MINIMUM_UPTIME_SECONDS = 60.0


def auto_restart_disabled() -> bool:
    """True when the environment opts out of the on-update restart."""
    return os.environ.get(ENV_NO_AUTO_RESTART, "") not in ("", "0", "false", "False")


def is_installed(distribution_name: str) -> bool:
    """Whether this package has installed metadata at all.

    The question ``install_fingerprint`` cannot answer, and the one that decides
    whether there is anything to watch: a bare source tree has no install to
    drift from and never will, while an install whose ``RECORD`` is briefly
    unreadable mid-upgrade is exactly the case the monitor exists for. Both read
    as a ``None`` fingerprint, so keying the decision on the fingerprint keys it
    on a proxy -- and gets the transient case permanently wrong.
    """
    try:
        distribution(distribution_name)
    except PackageNotFoundError:
        return False
    return True


def install_fingerprint(distribution_name: str) -> str | None:
    """Return the version plus a digest of the install's ``RECORD``.

    ``None`` means the fingerprint cannot be read right now: the package was
    never installed (a bare source tree), or an upgrade is mid-flight and the
    metadata is briefly gone. Callers must treat ``None`` as "wait", never as
    "changed".
    """
    try:
        installed = distribution(distribution_name)
        record = installed.read_text("RECORD")
        version = installed.version
    # TypeError joins the tuple because a half-removed dist-info raises it
    # rather than an OSError: `distribution()` matches on the directory name,
    # so it still resolves after METADATA is unlinked, and `.version` then
    # feeds None to email.message_from_string. Without this the watcher thread
    # dies for the life of the process and drift detection stops silently.
    except (PackageNotFoundError, OSError, TypeError) as exc:
        logger.debug("install fingerprint for %s is unreadable: %r", distribution_name, exc)
        return None
    # NOT `or ""`. read_text suppresses FileNotFoundError and returns None, so
    # coalescing turns "RECORD is absent" into sha256(b"") -- a present,
    # different, perfectly valid fingerprint. That is the exact case the
    # docstring above rules out, and a wheel writes RECORD last, so an ordinary
    # `uv tool install --upgrade` lands in that window. The result was a
    # restart fired against a half-written install, with no supervisor on POSIX
    # to bring the process back.
    if record is None:
        logger.debug("install fingerprint for %s: RECORD not readable yet", distribution_name)
        return None
    digest = hashlib.sha256(record.encode("utf-8")).hexdigest()[:16]
    return f"{version}:{digest}"


@dataclass
class _MonitorState:
    """One update monitor per process: its thread and the restart verdict.

    The handle outlives the thread so a second start is a no-op, and
    ``pending`` is what ``serve`` consults after the transport returns to
    tell a triggered restart apart from an ordinary shutdown.

    ``interrupt_owed`` is the second half of that, and it is one-shot on
    purpose. ``pending`` alone cannot say which signal ended this particular
    run: the monitor sets it asynchronously on its own timer, so an operator's
    Ctrl+C landing anywhere after it was set reads as the monitor's own signal
    and gets absorbed into a restart the operator did not ask for. The monitor
    raises exactly one SIGINT, so exactly one ``KeyboardInterrupt`` may be
    claimed as its own; every later one is a human and propagates.
    """

    thread: threading.Thread | None = None
    pending: bool = False
    interrupt_owed: bool = False
    started: float = 0.0


_MONITOR = _MonitorState()
# Guards every field of the state above. It began as the guard on the
# interrupt claim alone -- the monitor thread sets it and the main thread
# consumes it, so that read-modify-write cannot be a bare attribute test --
# and the start of the monitor itself is the same kind of read-modify-write
# by a second caller. `core/cache.py`'s `_AUTO_INDEX_RUNS` is the house
# pattern for both.
#
# One field is read without it on purpose: `_watch` reads `started` from
# inside the monitor thread. That read must stay unlocked, because the thread
# is started while the lock is held and taking it from the target would make
# the child wait on its own parent. The write happens before `start()`, so
# the thread cannot observe a zero and mistake a fresh process for one that
# has been up long enough to restart.
_MONITOR_LOCK = threading.Lock()


def restart_pending() -> bool:
    """True when the monitor shut the server down to restart it."""
    with _MONITOR_LOCK:
        return _MONITOR.pending


def claim_monitor_interrupt() -> bool:
    """Claim this ``KeyboardInterrupt`` as the monitor's own, once.

    Returns True only for the single interrupt the monitor itself raised.
    A second interrupt -- an operator pressing Ctrl+C while the restart is
    draining -- finds the claim spent and is answered False, so the human's
    shutdown wins over a restart already in flight rather than losing to it.
    """
    with _MONITOR_LOCK:
        owed = _MONITOR.interrupt_owed
        _MONITOR.interrupt_owed = False
        return owed


def start_update_monitor(distribution_name: str) -> threading.Thread | None:
    """Start one background thread watching the install; never blocks or raises.

    Returns the thread when one is watching, ``None`` when there is nothing
    to do: the environment opts out, a monitor is already running, or the
    package has no installed metadata to fingerprint (a bare source tree,
    which has no install to drift from).
    """
    if auto_restart_disabled():
        return None
    with _MONITOR_LOCK:
        running = _MONITOR.thread
    if running is not None:
        return running

    # Whether there is an install to drift from, not whether its fingerprint
    # reads right now: a server started by the very install event it should be
    # watching for -- which is exactly what a path-unit restart does -- can find
    # RECORD mid-rewrite, and treating that transient as "nothing to watch"
    # switched the feature off for the life of the process.
    if not is_installed(distribution_name):
        logger.info("install update monitor off: no installed metadata for %s", distribution_name)
        return None

    # Fingerprinted before the lock: reading RECORD is filesystem work, and a
    # second caller arriving during it should wait on the registry rather than
    # on the disk.
    baseline = install_fingerprint(distribution_name)

    with _MONITOR_LOCK:
        # Re-checked, because the two calls above released the lock.
        raced = _MONITOR.thread
        if raced is not None:
            return raced
        thread = threading.Thread(
            target=_watch,
            args=(distribution_name, baseline),
            name="install-update-monitor",
            daemon=True,
        )
        _MONITOR.started = time.monotonic()
        _MONITOR.thread = thread
        # Started under the lock, for the reason `core/cache.py` records: a
        # registered but unstarted thread reads as `is_alive() == False`, so
        # a caller probing in that window sees no monitor and starts a second
        # one. Two monitors means two SIGINTs for one install event, and only
        # one of them can be claimed.
        thread.start()
    return thread


def _watch(distribution_name: str, baseline: str | None) -> None:
    """Poll the fingerprint; on a real change, start the graceful shutdown.

    A ``None`` baseline means the fingerprint was unreadable at startup, so the
    first readable one establishes it instead. That is the same "None means
    wait, never changed" rule the poll below follows, applied to the one read
    that used to be exempt from it.
    """
    while True:
        time.sleep(POLL_SECONDS)
        current = install_fingerprint(distribution_name)
        if current is None:
            continue
        if baseline is None:
            baseline = current
            continue
        if current == baseline:
            continue
        if time.monotonic() - _MONITOR.started < MINIMUM_UPTIME_SECONDS:
            # Hold, do not discard. The change is real and must still be acted
            # on -- an upgrade landing seconds after startup is an ordinary
            # thing, and adopting its fingerprint as the new baseline would
            # leave this process serving the old code forever with no sign that
            # anything was missed. Waiting out the minimum uptime instead bounds
            # how often a restart can happen without ever losing one.
            logger.info(
                "install fingerprint changed %s -> %s within %.0fs of startup; holding the "
                "restart until the minimum uptime has passed",
                baseline,
                current,
                MINIMUM_UPTIME_SECONDS,
            )
            continue
        with _MONITOR_LOCK:
            _MONITOR.pending = True
            _MONITOR.interrupt_owed = True
        logger.info(
            "install updated (%s -> %s): restarting after graceful shutdown",
            baseline,
            current,
        )
        # The server's own shutdown path is the drain: SIGINT stops the
        # accept loop and finishes in-flight requests, then run() returns
        # and serve() consults restart_pending().
        signal.raise_signal(signal.SIGINT)
        return


def exec_or_exit() -> int:
    """Become the updated install, or finish the exit a supervisor completes.

    POSIX replaces the process image with the original argv -- same pid,
    same flags, new code. Windows returns 0 instead: exec with open handles
    is unreliable there, and a clean exit plus a supervisor ``Restart=`` is
    the documented completion of the loop.
    """
    # Nothing else joins the background workers, and exec runs no cleanup: a
    # grammar extraction killed half-written leaves a truncated library in a
    # cache every later process reads, with no atomic rename to fall back on.
    # The tag index needs no equivalent -- SQLite treats a hard kill as a crash
    # and rolls the transaction back on next open, and the write lock is an
    # flock on a close-on-exec descriptor, so exec releases it.
    grammars.wait_for_auto_warm()

    if platforms.family(sys.platform) == platforms.POSIX:
        argv = [sys.executable, *sys.orig_argv[1:]]
        logger.info("install updated: replacing this process with %s", " ".join(argv))
        try:
            os.execv(sys.executable, argv)
        except OSError:
            # The exec can fail for the same reasons that prompted it: the
            # upgrade replaced the venv, the interpreter moved, the path is
            # gone. Before this feature a drifting server kept serving stale
            # code; an unhandled OSError here would make it exit by traceback
            # instead, which is strictly less available than what it replaced.
            # Fall through to the clean exit a supervisor can act on.
            logger.exception(
                "install updated but exec of %s failed; exiting cleanly instead so a "
                "supervisor can restart this service",
                sys.executable,
            )
            return 0
    logger.info("install updated: exiting for the supervisor to restart with the new code")
    return 0
