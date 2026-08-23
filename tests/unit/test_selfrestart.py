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

    def test_a_missing_record_file_is_still_a_fingerprint(self, monkeypatch):
        # read_text returns None for a file the dist-info does not carry; the
        # version alone must still fingerprint rather than read as absent.
        monkeypatch.setattr(
            selfrestart, "distribution", lambda name: _FakeDistribution("1.0", None)
        )
        assert selfrestart.install_fingerprint("x") is not None


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
