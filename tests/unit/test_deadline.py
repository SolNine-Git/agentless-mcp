"""The per-call deadline: what stops a call, what extends it, and where it is checked.

No test here waits on a real clock. A deadline whose time is spent is built
with its start moved back, and the recache credit is measured against a fake
clock installed in the deadline module alone.
"""

import asyncio
from types import SimpleNamespace

import pytest

from agentless_mcp.application import symbol_service
from agentless_mcp.application.map_service import companions_for
from agentless_mcp.core import communities, graph, refs, resolve
from agentless_mcp.util import deadline
from agentless_mcp.util.errors import CallStopped

SOURCE = {
    "core.py": "def helper(value):\n    return value\n",
    "user.py": "from core import helper\n\n\ndef use(value):\n    return helper(value)\n",
    "tests/test_core.py": "from core import helper\n\n\ndef test_helper():\n    helper(1)\n",
}


def spent(limit=1.0, grace=0.0):
    """A deadline whose allowance ran out a minute ago."""
    expired = deadline.Deadline(limit=limit, grace=grace)
    expired.started -= limit + grace + 60
    return expired


class FakeClock:
    """A monotonic clock that moves only when a test says so."""

    def __init__(self):
        self.now = 1000.0

    def monotonic(self):
        return self.now


@pytest.fixture
def clock(monkeypatch):
    fake = FakeClock()
    monkeypatch.setattr(deadline, "time", SimpleNamespace(monotonic=fake.monotonic))
    return fake


class TestTheDeadline:
    def test_a_checkpoint_outside_a_call_does_nothing(self):
        assert deadline.active() is None
        deadline.checkpoint()

    def test_a_spent_allowance_stops_the_call(self):
        with deadline.bounded(spent(limit=5, grace=7)), pytest.raises(CallStopped) as stopped:
            deadline.checkpoint()

        assert "allows 5 s of work per call, plus up to 7 s" in str(stopped.value)

    def test_a_cancelled_call_stops_at_its_next_checkpoint(self):
        abandoned = deadline.Deadline(limit=60, grace=0)
        abandoned.cancel()

        with deadline.bounded(abandoned), pytest.raises(CallStopped, match="cancelled"):
            deadline.checkpoint()

    def test_parse_time_extends_the_allowance_up_to_the_grace(self, clock):
        call = deadline.Deadline(limit=10, grace=30, started=clock.now)

        with deadline.bounded(call):
            with deadline.recaching():
                clock.now += 20
            clock.now += 10
            deadline.checkpoint()
            with deadline.recaching():
                clock.now += 20
            with pytest.raises(CallStopped):
                deadline.checkpoint()

        assert call.credited == 30

    def test_recaching_outside_a_call_credits_nothing(self, clock):
        with deadline.recaching():
            clock.now += 5

    def test_the_active_deadline_reaches_a_worker_thread_and_is_reset_after(self):
        call = deadline.Deadline(limit=60, grace=0)

        async def seen_by_worker():
            with deadline.bounded(call):
                return await asyncio.to_thread(deadline.active)

        assert asyncio.run(seen_by_worker()) is call
        assert deadline.active() is None


class TestTheLongLoopsCheckIt:
    """Each loop that can run long stops under a spent deadline."""

    @pytest.fixture
    def built(self, tmp_path, extractor):
        for relative, text in SOURCE.items():
            (tmp_path / relative).parent.mkdir(parents=True, exist_ok=True)
            (tmp_path / relative).write_text(text, encoding="utf-8")
        scan = refs.scan_repo(tmp_path, extractor)
        index = refs.build_ref_index(scan)
        resolver, resolved = resolve.resolve_repo(scan, index)
        files = graph.build_graph(scan, index)
        return SimpleNamespace(
            root=tmp_path,
            scan=scan,
            index=index,
            resolver=resolver,
            resolved=resolved,
            files=files,
        )

    def stops(self, work):
        with deadline.bounded(spent()), pytest.raises(CallStopped):
            work()

    def test_the_scan(self, built, extractor):
        self.stops(lambda: refs.scan_repo(built.root, extractor))

    def test_reference_resolution(self, built):
        self.stops(lambda: resolve.build_graph(built.scan, built.resolver))

    def test_the_path_search(self, built):
        self.stops(lambda: resolve.shortest_path(built.resolved, "user.py", "core.py"))

    def test_the_file_graph(self, built):
        self.stops(lambda: graph.build_graph(built.scan, built.index))

    def test_the_ranking(self, built):
        self.stops(lambda: graph.personalized_pagerank(built.files))

    def test_the_community_passes(self, built):
        self.stops(lambda: communities.detect_communities(built.files))

    def test_the_test_companion_pass(self, built):
        by_path = built.scan.by_path()
        self.stops(lambda: companions_for(built.files, by_path, built.index, ["core.py"]))

    def test_the_shared_caller_ranking(self, built):
        definitions = list(refs.definitions_for(built.index, "helper"))
        sites = [site for entry in definitions for site in refs.references_to(built.index, entry)]
        by_path = built.scan.by_path()
        self.stops(
            lambda: symbol_service._shared_callers(
                sites, definitions, built.index, by_path, frozenset()
            )
        )
