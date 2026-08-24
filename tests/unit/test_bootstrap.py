"""The composition root: what it builds, and what it is allowed to build yet.

Selection itself is pinned next door in ``test_token_counter.py``. What is
pinned here is the wiring around it, which is where this module's own defects
live: the order in which argparse and the object graph get to fail, the
boundary the optional tokenizer is constructed behind, and the fact that both
entry points reach one selection point rather than two.
"""

import socket

import pytest

from agentless_mcp import bootstrap
from agentless_mcp.adapters.cli.formatting import EXIT_USAGE
from agentless_mcp.util.errors import AgentlessError, OperationFailed
from agentless_mcp.util.tokens import COUNTER_CHARS4, COUNTER_TIKTOKEN, Chars4Counter


class _ReachedError(Exception):
    """Sentinel: the counter was asked for, so stop before the graph is built."""


class _FailingTiktoken:
    """A ``tiktoken`` stand-in whose registry lookup fails.

    The real one reaches the network on a cold cache, which a unit test may
    not do. What is being pinned is the boundary, not the download.
    """

    def __init__(self, *, record=None):
        self.record = record if record is not None else []

    def get_encoding(self, name):
        self.record.append(socket.getdefaulttimeout())
        message = f"unknown encoding {name}"
        raise ValueError(message)


class TestNothingIsBuiltForAnInadmissibleArgv:
    """argparse decides first, because building the counter can itself fail.

    The pre-parse used to accept ``--token-counter`` in positions the full
    parser rejects, so a command line that was a usage error either way could
    still be answered with "install the tokens extra" -- about a flag the CLI
    does not take there at all.
    """

    def test_a_flag_the_full_parser_rejects_never_reaches_the_counter(self, monkeypatch):
        monkeypatch.setattr(bootstrap, "select_counter", self._must_not_build)

        with pytest.raises(SystemExit) as exit_info:
            bootstrap.cli_main(["tree", "--token-counter", COUNTER_CHARS4, "--repo", "/tmp"])

        assert exit_info.value.code == EXIT_USAGE

    def test_help_is_printed_before_a_counter_is_constructed(self, monkeypatch, capsys):
        monkeypatch.setattr(bootstrap, "select_counter", self._must_not_build)

        with pytest.raises(SystemExit) as exit_info:
            bootstrap.cli_main(["--token-counter", COUNTER_TIKTOKEN, "--help"])

        assert exit_info.value.code == 0
        assert "usage: agentless-mcp" in capsys.readouterr().out

    def test_an_argv_the_parser_accepts_reaches_the_counter(self, monkeypatch):
        """The control half: the gate above is a gate, not a wall."""
        seen = []

        def record(choice):
            seen.append(choice)
            raise _ReachedError

        monkeypatch.setattr(bootstrap, "select_counter", record)

        with pytest.raises(_ReachedError):
            bootstrap.cli_main(["map", "--repo", "/tmp"])

        assert seen == [None]

    @staticmethod
    def _must_not_build(choice):
        pytest.fail(f"a counter was built for {choice!r} before the argv was accepted")


class TestFailureReportingIsTheAdaptersOwn:
    """One spelling of "report and exit", not a second copy at the root."""

    def test_a_refused_counter_is_reported_with_the_usage_code(self, monkeypatch, capsys):
        def refuse(choice):
            message = "needs the 'tokens' extra, which is not installed"
            raise OperationFailed(message)

        monkeypatch.setattr(bootstrap, "select_counter", refuse)

        assert bootstrap.cli_main(["map", "--repo", "/tmp"]) == EXIT_USAGE
        assert capsys.readouterr().err == (
            "agentless-mcp: needs the 'tokens' extra, which is not installed\n"
        )

    def test_a_missing_mcp_extra_is_reported_the_same_way(self, monkeypatch, capsys):
        def absent(name):
            message = "no module named fastmcp"
            raise ImportError(message)

        monkeypatch.setattr(bootstrap.importlib, "import_module", absent)

        assert bootstrap.mcp_main([]) == EXIT_USAGE
        assert capsys.readouterr().err.startswith(
            "agentless-mcp: the MCP server needs the 'mcp' extra"
        )


class TestTheEncodingLoadIsInsideTheBoundary:
    """The one line in this module that reaches the network, and its bound.

    ``get_encoding`` sat outside the ``try`` that converts a missing extra,
    so an unknown encoding, an unwritable cache directory or a transport
    error left this module as whatever the library raised -- which
    ``cli_main`` does not catch, and which reaches the operator as a
    traceback instead of the install message.
    """

    def test_a_failed_encoding_load_leaves_as_the_error_the_cli_reports(self, monkeypatch):
        monkeypatch.setattr(bootstrap.importlib, "import_module", lambda name: _FailingTiktoken())

        with pytest.raises(AgentlessError, match=bootstrap.TIKTOKEN_ENCODING):
            bootstrap.TiktokenCounter()

    def test_the_load_runs_under_a_socket_bound(self, monkeypatch):
        record = []
        monkeypatch.setattr(
            bootstrap.importlib, "import_module", lambda name: _FailingTiktoken(record=record)
        )

        with pytest.raises(AgentlessError):
            bootstrap.TiktokenCounter()

        assert record == [bootstrap.TIKTOKEN_FETCH_TIMEOUT_SECONDS]

    def test_the_process_wide_default_is_put_back(self, monkeypatch):
        # The bound is set on the process, not on one call, so leaving it in
        # place would silently bound every socket opened afterwards.
        before = socket.getdefaulttimeout()
        monkeypatch.setattr(bootstrap.importlib, "import_module", lambda name: _FailingTiktoken())

        with pytest.raises(AgentlessError):
            bootstrap.TiktokenCounter()

        assert socket.getdefaulttimeout() == before


class _StubServer:
    """The MCP server module, reduced to the two names bootstrap uses."""

    ServerServices = dict

    @staticmethod
    def serve(argv, services):
        return 7


class TestTheServerReachesTheSameSelectionPoint:
    """Both entry points ask the same function what to count with.

    The server declares no ``--token-counter``, so its answer is always the
    chars/4 estimator. Naming the class here instead would be a second
    decision about a documented user-facing option, in the same file.
    """

    def test_the_server_asks_select_counter_with_no_choice(self, monkeypatch):
        chosen = []
        real = bootstrap.select_counter

        def record(choice):
            chosen.append(choice)
            return real(choice)

        monkeypatch.setattr(bootstrap, "select_counter", record)
        monkeypatch.setattr(bootstrap.importlib, "import_module", lambda name: _StubServer)

        assert bootstrap.mcp_main([]) == 7
        assert chosen == [None]

    def test_the_server_still_ends_up_with_the_estimator_the_pins_use(self, monkeypatch):
        built = []
        monkeypatch.setattr(bootstrap.importlib, "import_module", lambda name: _StubServer)
        monkeypatch.setattr(
            _StubServer, "ServerServices", lambda **fields: built.append(fields["counter"])
        )

        bootstrap.mcp_main([])

        assert isinstance(built[0], Chars4Counter)
