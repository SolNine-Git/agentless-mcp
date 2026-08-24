"""The three background registries and the parser they share.

`core/cache.py`'s `_AUTO_INDEX_RUNS` was the only one of the three written as
a real check-then-set under one lock. `core/grammars.py`'s `_AUTO_WARM` had no
lock at all and `core/selfrestart.py`'s `_MONITOR` guarded the interrupt claim
but not the start, so two callers arriving together could each find nothing
running and each start one. Two grammar warms write the same file into the
same cache directory; two install monitors raise two SIGINTs for one install
event, and only one of them can be claimed.

Hermetic by construction, as the house rule requires: every test replaces the
process-wide state object with a fresh one and stubs the thread body, so
nothing here depends on a real clock, on the ambient environment, on a
network fetch, or on what a previous test left behind. A barrier rather than a
sleep is what makes the callers actually collide.
"""

from __future__ import annotations

import threading

import pytest

from agentless_mcp.core import grammars, selfrestart

CALLERS = 12


def collide(work, callers: int = CALLERS):
    """Run ``work`` on ``callers`` threads released at the same instant.

    A barrier rather than a sleep: the window these guards protect is a few
    instructions wide, and a sleep long enough to be reliable would make the
    suite slow while still not proving the callers overlapped.
    """
    barrier = threading.Barrier(callers)
    results: list[object] = [None] * callers
    failures: list[BaseException] = []

    def run(index: int) -> None:
        try:
            barrier.wait()
            results[index] = work()
        except BaseException as error:  # noqa: BLE001 - reported, not swallowed
            failures.append(error)

    threads = [threading.Thread(target=run, args=(index,)) for index in range(callers)]
    for thread in threads:
        thread.start()
    for thread in threads:
        thread.join(timeout=30)

    assert not any(thread.is_alive() for thread in threads), "a caller never returned"
    assert not failures, f"callers raised: {failures}"
    return results


class TestParserIsolation:
    def test_each_caller_receives_its_own_parser(self):
        """One mutable Parser per caller, not one per process.

        `get_parser` was memoized, so four parse sites and the background
        index thread drove one object. Held in a list rather than compared by
        `id`, because a released object's id is free to be reused.
        """
        parsers = collide(lambda: grammars.get_parser("python"))
        assert len({id(parser) for parser in parsers}) == CALLERS

    def test_concurrent_parses_each_return_their_own_tree(self, extractor):
        """A guard for the pin, not a reproduction of a live failure.

        py-tree-sitter 0.26 holds the GIL for the whole of `parse`, so a
        shared parser does not corrupt today. The version pin is
        `>=0.25,<0.27` and a free-threaded build removes the GIL entirely, so
        what is safe is the version rather than the design. This asserts the
        property the design now provides on its own.
        """
        sources = [
            f"def function_{index}(value):\n    return {index}\n" for index in range(CALLERS)
        ]
        seen: list[list[str]] = [[] for _ in range(CALLERS)]
        barrier = threading.Barrier(CALLERS)

        def parse(index: int) -> None:
            barrier.wait()
            for _ in range(20):
                extracted = extractor.extract_from_source(sources[index], "python", f"m{index}.py")
                seen[index].extend(symbol.name for symbol in extracted)

        threads = [threading.Thread(target=parse, args=(index,)) for index in range(CALLERS)]
        for thread in threads:
            thread.start()
        for thread in threads:
            thread.join(timeout=30)

        for index, names in enumerate(seen):
            assert set(names) == {f"function_{index}"}, f"thread {index} saw {set(names)}"


class TestAutoWarmRegistry:
    @pytest.fixture(autouse=True)
    def isolated(self, monkeypatch):
        """A fresh registry, the kill switch lifted, and no real warming."""
        monkeypatch.setattr(grammars, "_AUTO_WARM", grammars._AutoWarmState())
        monkeypatch.delenv(grammars.ENV_NO_AUTO_WARM, raising=False)
        monkeypatch.setattr(grammars, "warmed_languages", frozenset)
        monkeypatch.setattr(grammars, "_auto_warm", lambda names: None)

    def test_twelve_callers_start_exactly_one_warm(self, monkeypatch):
        """Every caller is held between the check and the set at once.

        A barrier at the call site alone does not reproduce this: the
        function is short enough that the first caller finishes it inside one
        GIL slice, so the others never observe the empty registry. In the
        real path the gap is the cache probe -- filesystem work, which
        releases the GIL -- so the probe is where the barrier belongs. Twelve
        callers provably inside the window is what the lock has to survive.
        """
        window = threading.Barrier(CALLERS)

        def probe():
            window.wait(timeout=30)
            return frozenset()

        monkeypatch.setattr(grammars, "warmed_languages", probe)
        started = collide(lambda: grammars.start_auto_warm(["python"]))

        assert len({id(thread) for thread in started}) == 1
        assert started[0] is grammars._AUTO_WARM.thread

    def test_a_later_caller_is_answered_with_the_running_warm(self):
        first = grammars.start_auto_warm(["python"])
        assert grammars.start_auto_warm(["python"]) is first

    def test_the_deadline_is_published_before_the_thread_starts(self):
        # `_auto_warm` reads the deadline without the lock, which is only
        # sound because the write precedes `start()`. A zero deadline would
        # make the warm stop before warming anything.
        grammars.start_auto_warm(["python"])
        assert grammars._AUTO_WARM.deadline > 0


class TestMonitorRegistry:
    @pytest.fixture(autouse=True)
    def isolated(self, monkeypatch):
        """A fresh registry, the kill switch lifted, and no real watching."""
        monkeypatch.setattr(selfrestart, "_MONITOR", selfrestart._MonitorState())
        monkeypatch.delenv(selfrestart.ENV_NO_AUTO_RESTART, raising=False)
        monkeypatch.setattr(selfrestart, "is_installed", lambda name: True)
        monkeypatch.setattr(selfrestart, "install_fingerprint", lambda name: "0.0.0:abc")
        monkeypatch.setattr(selfrestart, "_watch", lambda name, baseline: None)

    def test_twelve_callers_start_exactly_one_monitor(self, monkeypatch):
        """The same window, held open at the same place, for the same reason.

        Here the real gap between the check and the set is the installed-
        metadata probe, which reads the `.dist-info` directory.
        """
        window = threading.Barrier(CALLERS)

        def probe(name):
            window.wait(timeout=30)
            return True

        monkeypatch.setattr(selfrestart, "is_installed", probe)
        started = collide(lambda: selfrestart.start_update_monitor("agentless-mcp"))

        assert len({id(thread) for thread in started}) == 1
        assert started[0] is selfrestart._MONITOR.thread

    def test_the_uptime_clock_is_published_before_the_thread_starts(self):
        # `_watch` reads `started` without the lock and compares it against
        # MINIMUM_UPTIME_SECONDS. A zero would make every fresh process look
        # like one that had been up long enough to restart itself.
        selfrestart.start_update_monitor("agentless-mcp")
        assert selfrestart._MONITOR.started > 0

    def test_starting_under_the_lock_does_not_deadlock_against_the_watcher(self, monkeypatch):
        """`_watch` takes the same lock the start holds, so prove it drains.

        `thread.start()` returns once the thread is scheduled rather than
        waiting for its body, so the child blocks at most until the starter
        releases. Pinned because the alternative -- holding a lock across a
        call into code that takes it -- is the shape that deadlocks, and the
        reason this one does not is worth stating.
        """
        entered = threading.Event()

        def watch(name, baseline):
            with selfrestart._MONITOR_LOCK:
                selfrestart._MONITOR.pending = True
            entered.set()

        monkeypatch.setattr(selfrestart, "_watch", watch)

        thread = selfrestart.start_update_monitor("agentless-mcp")
        assert thread is not None
        assert entered.wait(timeout=10), "the monitor thread never took the lock"
        assert selfrestart.restart_pending()
