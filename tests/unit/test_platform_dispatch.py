"""Platform dispatch for the two POSIX-only mechanisms, tested on Linux.

What is testable here is the *choice*: which family a platform string belongs
to, which Popen arguments that produces, and which kill path it takes. The
Windows system calls themselves are not testable off Windows, and a test that
monkeypatched ``msvcrt`` into existence would be testing its own stub. The
README says the Windows support is best effort for exactly this reason.

The POSIX side is tested for real, because this suite runs on it: the lock is
taken, a second acquisition is refused, and the refusal is a message naming
the repository rather than a wait.
"""

import errno
import importlib
import subprocess
import sys

import pytest

from agentless_mcp.core import cache, sandbox
from agentless_mcp.util import filelock, platforms
from agentless_mcp.util.errors import CacheLocked


class TestFamily:
    @pytest.mark.parametrize("platform", ["win32", "win64", "windows"])
    def test_windows_platform_strings_map_to_the_windows_family(self, platform):
        assert platforms.family(platform) == platforms.WINDOWS

    @pytest.mark.parametrize("platform", ["linux", "darwin", "freebsd14", "cygwin"])
    def test_everything_else_maps_to_posix(self, platform):
        assert platforms.family(platform) == platforms.POSIX

    def test_this_interpreter_is_classified(self):
        assert platforms.family(sys.platform) in (platforms.POSIX, platforms.WINDOWS)


class TestProcessGroupArguments:
    def test_posix_asks_for_a_new_session(self):
        assert sandbox._group_kwargs(platforms.POSIX) == {"start_new_session": True}

    def test_windows_asks_for_a_new_process_group(self):
        kwargs = sandbox._group_kwargs(platforms.WINDOWS)
        assert set(kwargs) == {"creationflags"}
        assert kwargs["creationflags"] == getattr(subprocess, "CREATE_NEW_PROCESS_GROUP", 0)

    def test_the_two_flavours_never_pass_the_same_argument(self):
        posix = sandbox._group_kwargs(platforms.POSIX)
        windows = sandbox._group_kwargs(platforms.WINDOWS)
        assert not set(posix) & set(windows)


class TestKillDispatch:
    def test_the_windows_flavour_signals_only_the_leader(self, monkeypatch):
        killed: list[str] = []
        monkeypatch.setattr(
            sandbox, "_kill_leader", lambda process: killed.append(f"leader:{process}")
        )
        monkeypatch.setattr(
            sandbox, "_signal_group", lambda group, number: killed.append(f"group:{group}:{number}")
        )

        sandbox._kill_group("fake-process", platforms.WINDOWS)

        assert killed == ["leader:fake-process"]

    def test_the_posix_flavour_signals_the_whole_group(self, monkeypatch):
        signalled: list[int] = []
        monkeypatch.setattr(
            sandbox, "_signal_group", lambda group, number: signalled.append(number)
        )

        process = subprocess.Popen([sys.executable, "-c", "pass"], start_new_session=True)
        try:
            sandbox._kill_group(process, platforms.POSIX)
        finally:
            process.wait(timeout=30)

        assert len(signalled) == 2  # SIGTERM then SIGKILL, unconditionally


class TestFileLock:
    def test_the_lock_is_exclusive_on_this_platform(self, tmp_path):
        flavour = platforms.family(sys.platform)
        path = tmp_path / "write.lock"

        def take_it_again():
            with filelock.exclusive(path, flavour=flavour):
                pytest.fail("the second acquisition should have been refused")

        with (
            filelock.exclusive(path, flavour=flavour),
            pytest.raises(filelock.LockUnavailableError),
        ):
            take_it_again()

    def test_taking_the_lock_does_not_truncate_the_file(self, tmp_path):
        flavour = platforms.family(sys.platform)
        path = tmp_path / "write.lock"
        path.write_text("held by someone\n", encoding="utf-8")

        with filelock.exclusive(path, flavour=flavour):
            pass

        assert path.read_text(encoding="utf-8") == "held by someone\n"

    @pytest.mark.skipif(
        platforms.family(sys.platform) != platforms.POSIX,
        reason="the fcntl branch only runs on POSIX",
    )
    def test_a_filesystem_that_cannot_lock_is_refused_not_leaked(self, tmp_path, monkeypatch):
        # ENOLCK is what a network or FUSE mount answers with; it must arrive
        # as the same typed refusal as a lock another process already holds.
        fcntl = importlib.import_module("fcntl")

        def no_locks(descriptor: int, operation: int) -> None:
            raise OSError(errno.ENOLCK, "No locks available")

        monkeypatch.setattr(fcntl, "flock", no_locks)

        with (
            pytest.raises(filelock.LockUnavailableError),
            filelock.exclusive(tmp_path / "write.lock", flavour=platforms.POSIX),
        ):
            pytest.fail("the acquisition should have been refused")

    def test_the_lock_is_released_for_the_next_holder(self, tmp_path):
        flavour = platforms.family(sys.platform)
        path = tmp_path / "write.lock"

        with filelock.exclusive(path, flavour=flavour):
            pass
        with filelock.exclusive(path, flavour=flavour):
            pass

    def test_the_index_reports_a_held_lock_by_repository(self, tmp_path, extractor):
        root = tmp_path / "repo"
        root.mkdir()
        (root / "core.py").write_text("def quote(sku):\n    return 1\n", encoding="utf-8")
        database = cache.cache_path(root)
        database.parent.mkdir(parents=True, exist_ok=True)

        flavour = platforms.family(sys.platform)
        with (
            filelock.exclusive(database.parent / cache.LOCK_NAME, flavour=flavour),
            pytest.raises(CacheLocked) as refusal,
        ):
            cache.build_index(root, extractor)

        assert str(root.resolve()) in str(refusal.value)
