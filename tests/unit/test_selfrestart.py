"""The on-update self-restart: fingerprinting, the monitor, and the exec seam.

The exec itself is patched everywhere here -- a test that really replaced the
pytest process would prove the feature by destroying the evidence. What is
provable in-process is the contract around it: what counts as a changed
install, what never triggers (absence, sameness), and that the restart goes
through the server's own SIGINT shutdown path rather than around it.
"""

import logging
import signal
import sys
import threading
from importlib.metadata import PackageNotFoundError

import pytest

from agentless_mcp.core import selfrestart


class _FakeDistribution:
    def __init__(self, version: str, record: str | None) -> None:
        self.version = version
        self._record = record

    def read_text(self, name: str) -> str | None:
        assert name == "RECORD"
        return self._record


@pytest.fixture
def monitor_isolated(monkeypatch):
    """A clean per-test monitor state, with the suite-wide opt-out lifted."""
    monkeypatch.delenv(selfrestart.ENV_NO_AUTO_RESTART, raising=False)
    monkeypatch.setattr(selfrestart, "_MONITOR", selfrestart._MonitorState())
    monkeypatch.setattr(selfrestart, "POLL_SECONDS", 0.01)


class TestFingerprint:
    def test_the_same_install_reads_as_the_same_fingerprint(self, monkeypatch):
        monkeypatch.setattr(
            selfrestart, "distribution", lambda name: _FakeDistribution("1.0", "a\nb\n")
        )
        assert selfrestart.install_fingerprint("x") == selfrestart.install_fingerprint("x")

    def test_a_record_change_and_a_version_bump_each_change_it(self, monkeypatch):
        prints = []
        for version, record in (("1.0", "a\n"), ("1.0", "b\n"), ("1.1", "a\n")):

            def install(name, v=version, r=record):
                return _FakeDistribution(v, r)

            monkeypatch.setattr(selfrestart, "distribution", install)
            prints.append(selfrestart.install_fingerprint("x"))
        assert len(set(prints)) == 3

    def test_no_installed_metadata_reads_as_absent(self, monkeypatch):
        def missing(name):
            raise PackageNotFoundError(name)

        monkeypatch.setattr(selfrestart, "distribution", missing)
        assert selfrestart.install_fingerprint("x") is None

    def test_an_unreadable_record_reads_as_absent_not_as_a_change(self, monkeypatch):
        # This replaces a test that asserted the opposite ("the version alone
        # must still fingerprint"). That assertion was the defect: read_text
        # suppresses FileNotFoundError and returns None, so the old `or ""`
        # fingerprinted an ABSENT record as sha256(b"") -- a present, different
        # value. A wheel writes RECORD last, so `uv tool install --upgrade`
        # opens exactly this window, and the monitor restarted against a
        # half-written install. The module docstring has always said absence
        # never triggers; the code now agrees with it.
        monkeypatch.setattr(
            selfrestart, "distribution", lambda name: _FakeDistribution("1.0", None)
        )
        assert selfrestart.install_fingerprint("x") is None

    def test_an_absent_record_is_not_mistaken_for_a_different_install(self, monkeypatch):
        # The regression stated as the operator sees it: the fingerprint taken
        # mid-upgrade must not compare unequal to the one taken before it.
        monkeypatch.setattr(
            selfrestart, "distribution", lambda name: _FakeDistribution("1.0", "a.py,,\n")
        )
        before = selfrestart.install_fingerprint("x")

        monkeypatch.setattr(
            selfrestart, "distribution", lambda name: _FakeDistribution("1.0", None)
        )
        mid_upgrade = selfrestart.install_fingerprint("x")

        assert before is not None
        assert mid_upgrade is None
        assert mid_upgrade != before or mid_upgrade is None

    def test_a_half_removed_dist_info_does_not_kill_the_watcher(self, monkeypatch):
        # `distribution()` matches on the directory name, so it still resolves
        # after METADATA is unlinked; `.version` then feeds None to
        # email.message_from_string and raises TypeError -- not an OSError.
        # Uncaught, that ends the daemon thread for the life of the process and
        # drift detection stops with only a stderr traceback.
        class HalfRemoved:
            def read_text(self, name: str) -> str | None:
                return None

            @property
            def version(self) -> str:
                message = "expected string or bytes-like object, got 'NoneType'"
                raise TypeError(message)

        monkeypatch.setattr(selfrestart, "distribution", lambda name: HalfRemoved())
        assert selfrestart.install_fingerprint("x") is None


class TestMonitor:
    def test_an_update_restarts_through_the_servers_own_shutdown(
        self, monkeypatch, monitor_isolated
    ):
        raised: list[int] = []
        monkeypatch.setattr(selfrestart.signal, "raise_signal", raised.append)
        # Baseline, one unchanged poll, one mid-upgrade absence, then the
        # updated install: only the last may trigger.
        answers = iter(["A", "A", None, "B"])
        monkeypatch.setattr(selfrestart, "install_fingerprint", lambda name: next(answers))

        monkeypatch.setattr(selfrestart, "is_installed", lambda name: True)
        monkeypatch.setattr(selfrestart, "MINIMUM_UPTIME_SECONDS", 0.0)

        thread = selfrestart.start_update_monitor("x")
        assert thread is not None
        # A second start is a no-op on the same thread: exactly one monitor.
        assert selfrestart.start_update_monitor("x") is thread

        thread.join(timeout=5)
        assert not thread.is_alive()
        assert selfrestart.restart_pending() is True
        assert raised == [signal.SIGINT]

    def test_the_environment_opt_out_keeps_the_monitor_off(self, monkeypatch, monitor_isolated):
        monkeypatch.setenv(selfrestart.ENV_NO_AUTO_RESTART, "1")
        monkeypatch.setattr(selfrestart, "install_fingerprint", self._must_not_fingerprint)
        assert selfrestart.start_update_monitor("x") is None
        assert selfrestart.restart_pending() is False

    def test_a_bare_source_tree_has_no_install_to_drift_from(
        self, monkeypatch, monitor_isolated, caplog
    ):
        monkeypatch.setattr(selfrestart, "install_fingerprint", lambda name: None)
        with caplog.at_level(logging.INFO, logger="agentless_mcp.core.selfrestart"):
            assert selfrestart.start_update_monitor("x") is None
        assert "install update monitor off" in caplog.text

    @staticmethod
    def _must_not_fingerprint(name):
        pytest.fail("the fingerprint must not be read here")


class TestExecOrExit:
    def test_posix_replaces_the_process_with_the_original_argv(self, monkeypatch):
        calls: list[tuple[str, list[str]]] = []
        # The real join reads a `grammars` module global, so a warm thread an
        # earlier test left running would make this test wait out the join
        # budget -- execution-order dependence this suite forbids.
        monkeypatch.setattr(selfrestart.grammars, "wait_for_auto_warm", lambda: None)
        monkeypatch.setattr(selfrestart.os, "execv", lambda exe, argv: calls.append((exe, argv)))
        monkeypatch.setattr(sys, "platform", "linux")

        selfrestart.exec_or_exit()

        assert calls == [(sys.executable, [sys.executable, *sys.orig_argv[1:]])]

    def test_windows_exits_for_the_supervisor_instead(self, monkeypatch):
        monkeypatch.setattr(selfrestart.os, "execv", self._must_not_exec)
        monkeypatch.setattr(sys, "platform", "win32")

        assert selfrestart.exec_or_exit() == 0

    @staticmethod
    def _must_not_exec(exe, argv):
        pytest.fail("exec must not happen on Windows")


class TestSuiteHermeticity:
    def test_the_suite_runs_with_the_monitor_opted_out(self):
        # tests/conftest.py sets the opt-out before any server spawns; a
        # monitor thread polling during unrelated tests would be exactly the
        # ambient-state dependence the suite forbids.
        assert selfrestart.auto_restart_disabled() is True


class TestInterruptOwnership:
    """Exactly one interrupt belongs to the monitor; every other one is a human.

    ``restart_pending`` says the monitor fired at some point, not that it raised
    the interrupt now being handled. Keying the absorb-or-propagate decision on
    it turned an operator's Ctrl+C during a draining restart into another
    restart.
    """

    def test_the_claim_is_false_before_the_monitor_fires(self, monitor_isolated):
        assert selfrestart.claim_monitor_interrupt() is False

    def test_the_monitors_own_interrupt_is_claimable_once(self, monitor_isolated):
        selfrestart._MONITOR.interrupt_owed = True

        assert selfrestart.claim_monitor_interrupt() is True
        assert selfrestart.claim_monitor_interrupt() is False

    def test_a_pending_restart_does_not_by_itself_grant_the_claim(self, monitor_isolated):
        # The monitor fired and its one signal was already consumed; a second
        # interrupt arriving now is the operator's.
        selfrestart._MONITOR.pending = True
        selfrestart._MONITOR.interrupt_owed = False

        assert selfrestart.restart_pending() is True
        assert selfrestart.claim_monitor_interrupt() is False


class TestExecFailureDoesNotKillTheServer:
    """A failed exec must not be worse than the drift it was fixing.

    Before this feature a drifting server kept serving stale code. An
    unhandled OSError out of the exec would make it exit by traceback, which
    is strictly less available than what it replaced.
    """

    def test_a_failing_exec_says_the_service_will_not_return_by_itself(self, monkeypatch, caplog):
        monkeypatch.setattr(selfrestart.grammars, "wait_for_auto_warm", lambda: None)
        monkeypatch.setattr(selfrestart.sys, "platform", "linux")

        def boom(executable, argv):
            raise OSError(2, "No such file or directory")

        monkeypatch.setattr(selfrestart.os, "execv", boom)

        with caplog.at_level(logging.ERROR, logger="agentless_mcp.core.selfrestart"):
            assert selfrestart.exec_or_exit() == 0

        assert "exec of" in caplog.text
        # The line an operator reads during an outage. The POSIX design needs
        # no supervisor while the exec succeeds; this is the path where it
        # does, and a line promising a supervisor will restart the service is
        # false on the deployment the module documents.
        assert "will NOT come back on its own" in caplog.text

    def test_the_grammar_warm_is_joined_before_the_image_is_replaced(self, monkeypatch):
        order = []
        monkeypatch.setattr(
            selfrestart.grammars, "wait_for_auto_warm", lambda: order.append("joined")
        )
        monkeypatch.setattr(selfrestart.sys, "platform", "linux")
        monkeypatch.setattr(selfrestart.os, "execv", lambda executable, argv: order.append("execv"))

        selfrestart.exec_or_exit()

        # An extraction killed half-written leaves a truncated library in a
        # cache every later process reads, and exec runs no cleanup.
        assert order == ["joined", "execv"]

    def test_the_windows_path_joins_the_warm_too(self, monkeypatch):
        order = []
        monkeypatch.setattr(
            selfrestart.grammars, "wait_for_auto_warm", lambda: order.append("joined")
        )
        monkeypatch.setattr(selfrestart.sys, "platform", "win32")
        monkeypatch.setattr(selfrestart.os, "execv", self._must_not_exec)

        assert selfrestart.exec_or_exit() == 0
        assert order == ["joined"]

    @staticmethod
    def _must_not_exec(executable, argv):
        message = "windows must exit for the supervisor rather than exec"
        raise AssertionError(message)


class TestBaselineDuringAnUpgrade:
    """An unreadable fingerprint at startup means wait, not "nothing to watch".

    A server started by the very install event it should watch for -- which is
    what a path-unit restart does -- can find RECORD mid-rewrite. Treating that
    transient as a bare source tree switched the feature off for the life of
    the process.
    """

    def test_an_installed_package_with_an_unreadable_record_is_still_watched(
        self, monkeypatch, monitor_isolated
    ):
        monkeypatch.setattr(selfrestart, "is_installed", lambda name: True)
        monkeypatch.setattr(selfrestart, "install_fingerprint", lambda name: None)
        # Stub the loop itself: a real monitor thread outliving this test would
        # keep polling into later ones and raise signals into their stubs.
        seen = []
        monkeypatch.setattr(selfrestart, "_watch", lambda name, baseline: seen.append(baseline))

        thread = selfrestart.start_update_monitor("agentless-mcp")

        assert thread is not None
        thread.join(timeout=5)
        # Watched with no baseline: the first readable fingerprint sets it.
        assert seen == [None]

    def test_a_bare_source_tree_is_still_not_watched(self, monkeypatch, monitor_isolated):
        monkeypatch.setattr(selfrestart, "is_installed", lambda name: False)

        assert selfrestart.start_update_monitor("agentless-mcp") is None

    def test_the_first_readable_fingerprint_becomes_the_baseline(
        self, monkeypatch, monitor_isolated
    ):
        raised = []
        monkeypatch.setattr(selfrestart.signal, "raise_signal", raised.append)
        monkeypatch.setattr(selfrestart, "MINIMUM_UPTIME_SECONDS", 0.0)
        reads = iter([None, "1.0:aaaa", "1.0:aaaa", "2.0:bbbb"])
        monkeypatch.setattr(
            selfrestart, "install_fingerprint", lambda name: next(reads, "2.0:bbbb")
        )

        selfrestart._watch("agentless-mcp", None)

        # The None was waited out, "1.0:aaaa" became the baseline, and only the
        # move away from it restarted.
        assert raised == [signal.SIGINT]
        assert selfrestart.restart_pending() is True
        assert selfrestart.claim_monitor_interrupt() is True


class _StopWatchingError(Exception):
    """Break out of the monitor's infinite poll loop from inside a stub."""


class TestMinimumUptime:
    """An early change is held until the minimum uptime, never discarded.

    Holding bounds how often a process can bounce. Discarding would lose the
    upgrade entirely: an install landing seconds after startup is ordinary, and
    adopting its fingerprint as the new baseline would leave this process
    serving the old code forever with nothing to show it had happened.
    """

    def test_a_change_inside_the_window_does_not_restart_yet(
        self, monkeypatch, monitor_isolated, caplog
    ):
        raised = []
        monkeypatch.setattr(selfrestart.signal, "raise_signal", raised.append)
        monkeypatch.setattr(selfrestart, "MINIMUM_UPTIME_SECONDS", 3600.0)
        selfrestart._MONITOR.started = selfrestart.time.monotonic()

        polls = []

        def fingerprint(name):
            polls.append(1)
            if len(polls) > 3:
                raise _StopWatchingError
            return "2.0:bbbb"

        monkeypatch.setattr(selfrestart, "install_fingerprint", fingerprint)

        with (
            caplog.at_level(logging.INFO, logger="agentless_mcp.core.selfrestart"),
            pytest.raises(_StopWatchingError),
        ):
            selfrestart._watch("agentless-mcp", "1.0:aaaa")

        assert raised == []
        assert selfrestart.restart_pending() is False
        assert "holding the restart" in caplog.text

    def test_the_held_change_still_restarts_once_the_window_passes(
        self, monkeypatch, monitor_isolated
    ):
        raised = []
        monkeypatch.setattr(selfrestart.signal, "raise_signal", raised.append)
        monkeypatch.setattr(selfrestart, "MINIMUM_UPTIME_SECONDS", 3600.0)
        # Started an hour ago as far as the guard is concerned: the same change
        # that was held above is now old enough to act on.
        selfrestart._MONITOR.started = selfrestart.time.monotonic() - 7200
        monkeypatch.setattr(selfrestart, "install_fingerprint", lambda name: "2.0:bbbb")

        selfrestart._watch("agentless-mcp", "1.0:aaaa")

        assert raised == [signal.SIGINT]
        assert selfrestart.restart_pending() is True

    def test_the_upgrade_is_never_adopted_as_the_new_baseline(self, monkeypatch, monitor_isolated):
        """The regression this guard must not become.

        Adopting the changed fingerprint while inside the window would make the
        difference vanish, so the restart would never fire even after the window
        passed. Uptime crosses the threshold mid-loop here; the change must
        still be there to act on.
        """
        raised = []
        monkeypatch.setattr(selfrestart.signal, "raise_signal", raised.append)
        monkeypatch.setattr(selfrestart, "MINIMUM_UPTIME_SECONDS", 3600.0)
        selfrestart._MONITOR.started = selfrestart.time.monotonic()

        polls = []

        def fingerprint(name):
            polls.append(1)
            if len(polls) == 3:
                # The window passes between polls.
                selfrestart._MONITOR.started = selfrestart.time.monotonic() - 7200
            return "2.0:bbbb"

        monkeypatch.setattr(selfrestart, "install_fingerprint", fingerprint)

        selfrestart._watch("agentless-mcp", "1.0:aaaa")

        assert raised == [signal.SIGINT]


class TestATornRecordIsAbsentRatherThanADifferentInstall:
    def test_a_record_that_is_not_utf8_reads_as_absent(self, monkeypatch):
        # `read_text` suppresses five OSError subclasses and nothing else, so a
        # RECORD caught mid-write with a byte sequence no UTF-8 decoder accepts
        # raises UnicodeDecodeError -- a ValueError, not an OSError. That is
        # the same torn-install window the monitor exists to watch, so it must
        # read as "wait", never as a change and never as a dead thread.
        class TornRecord:
            version = "1.0"

            def read_text(self, name: str) -> str:
                torn = UnicodeDecodeError("utf-8", b"\xff", 0, 1, "invalid start byte")
                raise torn

        monkeypatch.setattr(selfrestart, "distribution", lambda name: TornRecord())

        assert selfrestart.install_fingerprint("x") is None


class TestADeadMonitorDoesNotReadAsARunningOne:
    """`_MONITOR.thread` is the early return in ``start_update_monitor``.

    A thread ended by an exception no caught type covers used to leave that
    handle set, so every later call was answered with the corpse and drift
    detection stayed off for the life of the process -- ``is_alive()`` False,
    and nothing consulting it.
    """

    def test_a_reader_error_no_tuple_names_deregisters_the_monitor(
        self, monkeypatch, monitor_isolated
    ):
        def explode(name):
            raise _StopWatchingError

        monkeypatch.setattr(selfrestart, "install_fingerprint", explode)
        selfrestart._MONITOR.thread = threading.current_thread()

        with pytest.raises(_StopWatchingError):
            selfrestart._watch("agentless-mcp", None)

        assert selfrestart._MONITOR.thread is None

    def test_the_next_start_gets_a_fresh_monitor_rather_than_the_corpse(
        self, monkeypatch, monitor_isolated
    ):
        monkeypatch.setattr(selfrestart, "is_installed", lambda name: True)
        monkeypatch.setattr(selfrestart, "install_fingerprint", lambda name: "1.0:aaaa")
        started = []
        monkeypatch.setattr(selfrestart, "_watch", lambda name, baseline: started.append(name))

        first = selfrestart.start_update_monitor("agentless-mcp")
        assert first is not None
        first.join(timeout=5)
        # What the dead thread leaves behind, which the finally in `_watch`
        # does for real and the stub above cannot.
        selfrestart._MONITOR.thread = None

        second = selfrestart.start_update_monitor("agentless-mcp")

        assert second is not None
        second.join(timeout=5)
        assert second is not first
        assert started == ["agentless-mcp", "agentless-mcp"]

    def test_a_monitor_that_armed_a_restart_stays_registered(self, monkeypatch, monitor_isolated):
        # It finished the job it was started for and the process is on its way
        # out. Forgetting it there would let a second monitor start and raise a
        # second SIGINT for one install event, and only one can be claimed.
        monkeypatch.setattr(selfrestart.signal, "raise_signal", lambda number: None)
        monkeypatch.setattr(selfrestart, "MINIMUM_UPTIME_SECONDS", 0.0)
        reads = iter(["1.0:aaaa", "2.0:bbbb"])
        monkeypatch.setattr(selfrestart, "install_fingerprint", lambda name: next(reads))
        selfrestart._MONITOR.thread = threading.current_thread()

        selfrestart._watch("agentless-mcp", None)

        assert selfrestart._MONITOR.pending is True
        assert selfrestart._MONITOR.thread is not None
