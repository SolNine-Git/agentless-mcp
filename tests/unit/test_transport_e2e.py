"""End-to-end transport tests: a real server subprocess on each shipped transport.

Every other MCP test speaks to the server through FastMCP's in-memory
transport, which proves registration and dispatch but not the wire. These
tests spawn the installed ``agentless-mcp-server`` console script, complete
the MCP initialize handshake over each shipped transport, assert the
advertised tool set matches the in-memory listing, and make one bounded call.
This is the gate docs/analysis/archive/functional-assessment.md (M3) found
missing when the packaged stdio server hung undetected under the locked
dependency stack.

Every subprocess interaction is bounded well below pytest-timeout's 60s
ceiling, so a hang fails inside the test with the server's captured stderr
in the failure message rather than as a bare suite timeout.
"""

import asyncio
import socket
import subprocess
import sys
import time
from pathlib import Path

import pytest
from fastmcp import Client
from fastmcp.client.transports import StdioTransport, StreamableHttpTransport

from agentless_mcp.adapters.mcp.server import ServerServices, ToolHandlers, build_server
from agentless_mcp.application.graph_service import GraphService
from agentless_mcp.application.map_service import MapService
from agentless_mcp.application.symbol_service import SymbolService
from agentless_mcp.application.view_service import ViewService

pytestmark = pytest.mark.e2e

LOOPBACK = "127.0.0.1"

# One budget for the whole client conversation (handshake, tool listing, one
# call) and tighter ones for startup and shutdown. All deliberately below the
# suite's 60s pytest-timeout so a wedged server fails here, stderr attached.
CONVERSATION_TIMEOUT = 20.0
STARTUP_TIMEOUT = 15.0
SHUTDOWN_TIMEOUT = 10.0

SOURCE = "def quote(sku):\n    return 1\n"


@pytest.fixture
def fixture_repo(tmp_path):
    """A one-file repository; python is in the session's warmed grammar set."""
    root = tmp_path / "alpha"
    root.mkdir()
    (root / "core.py").write_text(SOURCE, encoding="utf-8")
    return root


@pytest.fixture
def server_script():
    """The installed console script, found next to the running interpreter.

    The script rather than ``python -m``: there is no module spelling of the
    server entry point, and the console script is exactly what the README
    tells an operator to configure, so it is the wiring worth testing.
    """
    script = Path(sys.executable).parent / "agentless-mcp-server"
    if not script.exists():
        pytest.fail(f"agentless-mcp-server is not installed next to {sys.executable}")
    return str(script)


@pytest.fixture
def in_memory_tools(extractor, counter, fixture_repo):
    """The tool set the in-memory transport advertises, listed live.

    Computed rather than copied from a constant so the assertion is
    "subprocess equals in-memory", not "both equal a third list".
    """
    services = ServerServices(
        maps=MapService(extractor, counter),
        views=ViewService(extractor),
        symbols=SymbolService(extractor, counter),
        graphs=GraphService(extractor),
        counter=counter,
        extractor=extractor,
    )
    server = build_server(ToolHandlers([fixture_repo], services))

    async def listing():
        async with Client(server) as client:
            return {tool.name for tool in await client.list_tools()}

    return asyncio.run(asyncio.wait_for(listing(), timeout=CONVERSATION_TIMEOUT))


async def _conversation(client):
    """Complete the handshake, list the tools, and make one cheap call."""
    async with client:
        tools = {tool.name for tool in await client.list_tools()}
        result = await client.call_tool("capabilities", {})
        return tools, result.content[0].text


def _run_bounded(client, describe_stderr):
    """Run one conversation under the budget.

    Whatever the client library raises -- a timeout, a broken pipe, a protocol
    error -- propagates with its real class, and the finally block emits the
    server's stderr into the test's captured output so the failure report
    carries it. A blind except would hide the failure class to say the same.
    """
    completed = False
    try:
        result = asyncio.run(asyncio.wait_for(_conversation(client), CONVERSATION_TIMEOUT))
        completed = True
        return result
    finally:
        if not completed:
            sys.stderr.write(f"server stderr:\n{describe_stderr()}\n")


class TestStdio:
    def test_a_spawned_stdio_server_serves_the_in_memory_tool_set(
        self, server_script, fixture_repo, tmp_path, in_memory_tools
    ):
        log_path = tmp_path / "server-stderr.log"
        transport = StdioTransport(
            command=server_script,
            args=["--root", str(fixture_repo)],
            log_file=log_path,
            # The subprocess must die with the session; keep_alive would leave
            # it running past the test.
            keep_alive=False,
        )

        def stderr_text():
            if log_path.exists():
                return log_path.read_text(encoding="utf-8", errors="replace")
            return "<no stderr captured>"

        tools, body = _run_bounded(Client(transport), stderr_text)

        assert tools == in_memory_tools, f"server stderr:\n{stderr_text()}"
        assert body.startswith("// agentless-mcp receipt"), body


class TestHttp:
    def test_a_spawned_http_server_serves_the_in_memory_tool_set(
        self, server_script, fixture_repo, in_memory_tools
    ):
        port = _free_port()
        process = subprocess.Popen(
            [
                server_script,
                "--transport",
                "http",
                "--host",
                LOOPBACK,
                "--port",
                str(port),
                "--root",
                str(fixture_repo),
            ],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.PIPE,
            text=True,
        )
        completed = False
        try:
            assert _listens(process, port), (
                f"server exited (code {process.poll()}) or never listened on "
                f"{LOOPBACK}:{port} within {STARTUP_TIMEOUT}s"
            )
            url = f"http://{LOOPBACK}:{port}/mcp/"
            tools, body = asyncio.run(
                asyncio.wait_for(
                    _conversation(Client(StreamableHttpTransport(url))),
                    CONVERSATION_TIMEOUT,
                )
            )
            completed = True
        finally:
            # Always torn down, bounded; on failure the server's stderr goes
            # into the test's captured output so the report carries it.
            stderr = _drain(process)
            if not completed:
                sys.stderr.write(f"server stderr:\n{stderr}\n")

        assert tools == in_memory_tools, f"server stderr:\n{stderr}"
        assert body.startswith("// agentless-mcp receipt"), body


def _free_port():
    """A loopback port that was free at probe time."""
    with socket.socket() as sock:
        sock.bind((LOOPBACK, 0))
        return sock.getsockname()[1]


def _listens(process, port):
    """Wait, bounded, until the port accepts or the server exits."""
    deadline = time.monotonic() + STARTUP_TIMEOUT
    while time.monotonic() < deadline:
        if process.poll() is not None:
            return False
        try:
            with socket.create_connection((LOOPBACK, port), timeout=1.0):
                return True
        except OSError:
            time.sleep(0.05)
    return False


def _drain(process):
    """Stop the server and return its stderr, with a hard bound on every wait."""
    if process.poll() is None:
        process.terminate()
    try:
        _, stderr = process.communicate(timeout=SHUTDOWN_TIMEOUT)
    except subprocess.TimeoutExpired:
        process.kill()
        _, stderr = process.communicate(timeout=SHUTDOWN_TIMEOUT)
    return stderr
