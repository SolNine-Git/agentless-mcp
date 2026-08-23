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

from agentless_mcp.util import platforms

logger = logging.getLogger(__name__)

ENV_NO_AUTO_RESTART = "AGENTLESS_MCP_NO_AUTO_RESTART"

# Slow on purpose: an upgrade is a human-timescale event, and the only cost
# of a late restart is a few more answers from the old code. Two reads of a
# metadata file every interval is noise next to one tool call.
POLL_SECONDS = 30.0


def auto_restart_disabled() -> bool:
    """True when the environment opts out of the on-update restart."""
    return os.environ.get(ENV_NO_AUTO_RESTART, "") not in ("", "0", "false", "False")


def install_fingerprint(distribution_name: str) -> str | None:
    """Return the version plus a digest of the install's ``RECORD``.

    ``None`` means the fingerprint cannot be read right now: the package was
    never installed (a bare source tree), or an upgrade is mid-flight and the
    metadata is briefly gone. Callers must treat ``None`` as "wait", never as
    "changed".
    """
    try:
        installed = distribution(distribution_name)
        record = installed.read_text("RECORD") or ""
        version = installed.version
    except (PackageNotFoundError, OSError):
        return None
    digest = hashlib.sha256(record.encode("utf-8")).hexdigest()[:16]
    return f"{version}:{digest}"


@dataclass
class _MonitorState:
    """One update monitor per process: its thread and the restart verdict.

    The handle outlives the thread so a second start is a no-op, and
    ``pending`` is what ``serve`` consults after the transport returns to
    tell a triggered restart apart from an ordinary shutdown.
    """

    thread: threading.Thread | None = None
    pending: bool = False


_MONITOR = _MonitorState()


def restart_pending() -> bool:
    """True when the monitor shut the server down to restart it."""
    return _MONITOR.pending


def start_update_monitor(distribution_name: str) -> threading.Thread | None:
    """Start one background thread watching the install; never blocks or raises.

    Returns the thread when one is watching, ``None`` when there is nothing
    to do: the environment opts out, a monitor is already running, or the
    package has no installed metadata to fingerprint (a bare source tree,
    which has no install to drift from).
    """
    if auto_restart_disabled():
        return None
    if _MONITOR.thread is not None:
        return _MONITOR.thread

    baseline = install_fingerprint(distribution_name)
    if baseline is None:
        logger.info("install update monitor off: no installed metadata for %s", distribution_name)
        return None

    thread = threading.Thread(
        target=_watch,
        args=(distribution_name, baseline),
        name="install-update-monitor",
        daemon=True,
    )
    _MONITOR.thread = thread
    thread.start()
    return thread


def _watch(distribution_name: str, baseline: str) -> None:
    """Poll the fingerprint; on a real change, start the graceful shutdown."""
    while True:
        time.sleep(POLL_SECONDS)
        current = install_fingerprint(distribution_name)
        if current is None or current == baseline:
            continue
        _MONITOR.pending = True
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
    if platforms.family(sys.platform) == platforms.POSIX:
        argv = [sys.executable, *sys.orig_argv[1:]]
        logger.info("install updated: replacing this process with %s", " ".join(argv))
        os.execv(sys.executable, argv)
    logger.info("install updated: exiting for the supervisor to restart with the new code")
    return 0
