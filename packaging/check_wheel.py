"""Check an installed agentless-mcp wheel: its two commands, and one MCP session per transport."""

from __future__ import annotations

import argparse
import http.client
import json
import os
import selectors
import signal
import socket
import subprocess
import sys
import tempfile
import time
from collections.abc import Callable, Iterator
from pathlib import Path
from types import FrameType
from typing import Any, NoReturn, TypeGuard

import tomllib

ROOT = Path(__file__).resolve().parent.parent
TOOLS = frozenset(
    {"capabilities", "find_referencing_symbols", "graph", "history", "orient", "read", "symbols"}
)
SOURCE = "def quote(sku):\n    return 1\n\n\ndef total(items):\n    return sum(map(quote, items))\n"
NOTICES = (
    "LICENSE",
    "NOTICE",
    "THIRD-PARTY-CRATES.md",
    "grammars/MANIFEST.toml",
    "grammars/shim/utf8proc/LICENSE.md",
)
LOOPBACK = "127.0.0.1"
PROTOCOL = "2025-06-18"
STEP_SECONDS = 20.0
START_SECONDS = 15.0
STOP_SECONDS = 10.0
HTTP_OK = 200
HTTP_ACCEPTED = 202

Send = Callable[[str, dict[str, Any], int | None], dict[str, Any] | None]


class CheckFailedError(Exception):
    """A check of the installed wheel failed."""


def main(argv: list[str] | None = None) -> int:
    """Run one check against the commands in --bin-dir; exit 0 when it passes, 1 when it fails."""
    for number in (signal.SIGTERM, signal.SIGHUP):
        signal.signal(number, leave)
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("check", choices=["smoke", "e2e"])
    parser.add_argument("--bin-dir", type=Path, required=True)
    args = parser.parse_args(argv)
    with tempfile.TemporaryDirectory() as scratch:
        env = isolated_env(Path(scratch))
        repo = make_repo(Path(scratch) / "repo", env)
        try:
            if args.check == "smoke":
                smoke(args.bin_dir, repo, env)
            else:
                e2e(args.bin_dir, repo, Path(scratch))
        except CheckFailedError as failure:
            print(f"check_wheel {args.check}: {failure}", file=sys.stderr)
            return 1
    print(f"check_wheel {args.check}: passed")
    return 0


# The signal ends the check through SystemExit, so each session's finally stops its server.
def leave(number: int, _frame: FrameType | None) -> NoReturn:
    raise SystemExit(128 + number)


def isolated_env(scratch: Path) -> dict[str, str]:
    env = dict(os.environ)
    env.update(
        HOME=str(scratch / "home"),
        XDG_CACHE_HOME=str(scratch / "cache"),
        XDG_CONFIG_HOME=str(scratch / "config"),
        GIT_CONFIG_GLOBAL=os.devnull,
        GIT_CONFIG_NOSYSTEM="1",
    )
    return env


def make_repo(repo: Path, env: dict[str, str]) -> Path:
    repo.mkdir()
    (repo / "core.py").write_text(SOURCE, encoding="utf-8")
    identity = ["-c", "user.name=check", "-c", "user.email=check@example.invalid"]
    for args in (
        ["init", "--quiet"],
        ["add", "core.py"],
        [*identity, "commit", "--quiet", "-m", "Add the checked file", "-m", "The map reads it."],
    ):
        run(["git", "-C", str(repo), *args], env)
    return repo


def run(command: list[str], env: dict[str, str]) -> str:
    try:
        done = subprocess.run(
            command, env=env, capture_output=True, text=True, timeout=STEP_SECONDS, check=False
        )
    except subprocess.TimeoutExpired as expired:
        msg = f"{command} did not exit within {STEP_SECONDS} s"
        raise CheckFailedError(msg) from expired
    if done.returncode != 0:
        msg = f"{command} exited {done.returncode}: {done.stderr.strip()}"
        raise CheckFailedError(msg)
    return done.stdout


def smoke(bin_dir: Path, repo: Path, env: dict[str, str]) -> None:
    version = cargo_version()
    said = run([str(bin_dir / "agentless-mcp-server"), "--version"], env)
    if said.split() != ["agentless-mcp-server", version]:
        msg = f"agentless-mcp-server --version printed {said!r}, not version {version}"
        raise CheckFailedError(msg)
    notices(bin_dir)
    mapped = run([str(bin_dir / "agentless-mcp"), "map", "--repo", str(repo)], env)
    if "core.py" not in mapped:
        msg = f"agentless-mcp map did not list core.py: {mapped!r}"
        raise CheckFailedError(msg)


# maturin skips a license-files glob that matches nothing, so this checks the installed notices.
def notices(bin_dir: Path) -> None:
    pattern = "lib/python*/site-packages/agentless_mcp-*.dist-info/licenses/notices"
    found = sorted(bin_dir.parent.glob(pattern))
    if len(found) != 1:
        msg = f"{len(found)} directories match {pattern} under {bin_dir.parent}"
        raise CheckFailedError(msg)
    missing = [name for name in NOTICES if not (found[0] / name).is_file()]
    if missing:
        msg = f"the installed notices lack {missing}"
        raise CheckFailedError(msg)


def cargo_version() -> str:
    with (ROOT / "Cargo.toml").open("rb") as manifest:
        return str(tomllib.load(manifest)["workspace"]["package"]["version"])


def e2e(bin_dir: Path, repo: Path, scratch: Path) -> None:
    env = isolated_env(scratch)
    server = bin_dir / "agentless-mcp-server"
    over_stdio = stdio_session(server, repo, env, scratch / "stdio.log")
    over_http = http_session(server, repo, env, scratch / "http.log")
    if over_stdio != over_http:
        msg = "the stdio and HTTP sessions listed different tools"
        raise CheckFailedError(msg)


def conversation(send: Send) -> list[Any]:
    initialize = {
        "protocolVersion": PROTOCOL,
        "capabilities": {},
        "clientInfo": {"name": "check_wheel", "version": "1"},
    }
    field(send("initialize", initialize, 1), "result", "serverInfo")
    send("notifications/initialized", {}, None)
    tools = field(send("tools/list", {}, 2), "result", "tools")
    names = {field(tool, "name") for tool in tools}
    if names != TOOLS:
        msg = f"tools/list named {sorted(names)}, not {sorted(TOOLS)}"
        raise CheckFailedError(msg)
    call = {"name": "orient", "arguments": {"operation": "map"}}
    result = field(send("tools/call", call, 3), "result")
    text = "".join(field(part, "text") for part in field(result, "content"))
    if result.get("isError") is not False or "core.py" not in text:
        msg = f"orient map answered {result!r}"
        raise CheckFailedError(msg)
    return list(tools)


def encode(method: str, params: dict[str, Any], ident: int | None) -> bytes:
    message: dict[str, Any] = {"jsonrpc": "2.0", "method": method, "params": params}
    if ident is not None:
        message["id"] = ident
    return json.dumps(message).encode()


def field(message: Any, *keys: str) -> Any:
    value = message
    for key in keys:
        if not isinstance(value, dict) or key not in value:
            msg = f"no {'.'.join(keys)} in {message!r}"
            raise CheckFailedError(msg)
        value = value[key]
    return value


def stdio_session(server: Path, repo: Path, env: dict[str, str], log: Path) -> list[Any]:
    with log.open("wb") as stderr:
        process = subprocess.Popen(
            [str(server), "--root", str(repo)],
            env=env,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=stderr,
        )
        try:
            tools = conversation(StdioClient(process).send)
            stop_stdio(process)
        except CheckFailedError as failure:
            msg = f"stdio: {failure}; server stderr: {log.read_text(errors='replace')}"
            raise CheckFailedError(msg) from failure
        finally:
            kill_if_running(process)
    return tools


def stop_stdio(process: subprocess.Popen[bytes]) -> None:
    if process.stdin is not None:
        process.stdin.close()
    try:
        code = process.wait(timeout=STOP_SECONDS)
    except subprocess.TimeoutExpired as expired:
        msg = f"the server did not exit within {STOP_SECONDS} s of the end of input"
        raise CheckFailedError(msg) from expired
    if code != 0:
        msg = f"the server exited {code} at the end of input"
        raise CheckFailedError(msg)


def kill_if_running(process: subprocess.Popen[bytes]) -> None:
    if process.poll() is None:
        process.kill()
        process.wait(timeout=STOP_SECONDS)


class StdioClient:
    """One JSON-RPC message per line on the server's standard input and output."""

    def __init__(self, process: subprocess.Popen[bytes]) -> None:
        if process.stdin is None or process.stdout is None:
            msg = "the server has no pipes"
            raise CheckFailedError(msg)
        self.stdin = process.stdin
        self.stdout = process.stdout
        self.buffer = b""

    def send(self, method: str, params: dict[str, Any], ident: int | None) -> dict[str, Any] | None:
        try:
            self.stdin.write(encode(method, params, ident) + b"\n")
            self.stdin.flush()
        except OSError as error:
            msg = f"{method}: cannot write to the server: {error}"
            raise CheckFailedError(msg) from error
        if ident is None:
            return None
        deadline = time.monotonic() + STEP_SECONDS
        while True:
            answer = decode(self.line(deadline))
            if is_answer(answer, ident):
                return answer

    def line(self, deadline: float) -> bytes:
        with selectors.DefaultSelector() as selector:
            selector.register(self.stdout, selectors.EVENT_READ)
            while b"\n" not in self.buffer:
                remaining = deadline - time.monotonic()
                if remaining <= 0 or not selector.select(remaining):
                    msg = f"no answer within {STEP_SECONDS} s"
                    raise CheckFailedError(msg)
                chunk = os.read(self.stdout.fileno(), 65536)
                if not chunk:
                    msg = "the server closed its output"
                    raise CheckFailedError(msg)
                self.buffer += chunk
        line, _, self.buffer = self.buffer.partition(b"\n")
        return line


def http_session(server: Path, repo: Path, env: dict[str, str], log: Path) -> list[Any]:
    port = free_port()
    command = [str(server), "--transport", "http", "--host", LOOPBACK, "--port", str(port)]
    with log.open("wb") as output:
        process = subprocess.Popen(
            [*command, "--root", str(repo)], env=env, stdout=output, stderr=output
        )
        try:
            wait_listening(process, port)
            tools = conversation(HttpClient(port).send)
            stop_http(process)
        except CheckFailedError as failure:
            msg = f"HTTP: {failure}; server output: {log.read_text(errors='replace')}"
            raise CheckFailedError(msg) from failure
        finally:
            kill_if_running(process)
    return tools


def free_port() -> int:
    with socket.socket() as probe:
        probe.bind((LOOPBACK, 0))
        return int(probe.getsockname()[1])


def wait_listening(process: subprocess.Popen[bytes], port: int) -> None:
    deadline = time.monotonic() + START_SECONDS
    while time.monotonic() < deadline:
        if process.poll() is not None:
            msg = f"the server exited {process.returncode} before it listened"
            raise CheckFailedError(msg)
        try:
            socket.create_connection((LOOPBACK, port), timeout=1.0).close()
        except OSError:
            time.sleep(0.1)
        else:
            return
    msg = f"the server did not listen on port {port} within {START_SECONDS} s"
    raise CheckFailedError(msg)


def stop_http(process: subprocess.Popen[bytes]) -> None:
    process.send_signal(signal.SIGTERM)
    try:
        code = process.wait(timeout=STOP_SECONDS)
    except subprocess.TimeoutExpired as expired:
        msg = f"the server did not exit within {STOP_SECONDS} s of SIGTERM"
        raise CheckFailedError(msg) from expired
    # A graceful stop exits 0, or ends by the signal again after its shutdown, as uvicorn does.
    if code not in {0, -signal.SIGTERM}:
        msg = f"the server exited {code} on SIGTERM"
        raise CheckFailedError(msg)


class HttpClient:
    """Streamable HTTP: one POST per message, the answer as JSON or as server-sent events."""

    def __init__(self, port: int) -> None:
        self.port = port
        self.session: str | None = None

    def send(self, method: str, params: dict[str, Any], ident: int | None) -> dict[str, Any] | None:
        connection = http.client.HTTPConnection(LOOPBACK, self.port, timeout=STEP_SECONDS)
        try:
            connection.request("POST", "/mcp", encode(method, params, ident), self.headers())
            response = connection.getresponse()
            self.session = response.getheader("mcp-session-id", self.session)
            return self.answer(response, ident, connection.sock)
        except (OSError, http.client.HTTPException) as error:
            msg = f"{method}: {error}"
            raise CheckFailedError(msg) from error
        finally:
            connection.close()

    def headers(self) -> dict[str, str]:
        headers = {
            "Content-Type": "application/json",
            "Accept": "application/json, text/event-stream",
        }
        if self.session is not None:
            headers["Mcp-Session-Id"] = self.session
            headers["Mcp-Protocol-Version"] = PROTOCOL
        return headers

    def answer(
        self, response: http.client.HTTPResponse, ident: int | None, sock: socket.socket
    ) -> dict[str, Any] | None:
        if ident is None:
            if response.status != HTTP_ACCEPTED:
                msg = f"a notification got status {response.status}"
                raise CheckFailedError(msg)
            return None
        if response.status != HTTP_OK:
            msg = f"status {response.status}: {response.read()!r}"
            raise CheckFailedError(msg)
        if response.getheader("Content-Type", "").startswith("text/event-stream"):
            return event_answer(response, ident, sock)
        answer = decode(response.read())
        if not is_answer(answer, ident):
            msg = f"the body {answer!r} does not answer request {ident}"
            raise CheckFailedError(msg)
        return answer


# A blank line ends each server-sent event. An event with no data (a priming event) answers nothing.
def event_answer(
    response: http.client.HTTPResponse, ident: int, sock: socket.socket
) -> dict[str, Any]:
    data: list[bytes] = []
    for line in event_lines(response, sock, ident):
        if line.startswith(b"data:"):
            data.append(line[len(b"data:") :].removeprefix(b" "))
        elif not line:
            event, data = b"\n".join(data), []
            answer = decode(event) if event else None
            if is_answer(answer, ident):
                return answer
    msg = f"the event stream ended without the answer to request {ident}"
    raise CheckFailedError(msg)


# One deadline bounds the whole stream, so keep-alive comments cannot extend the read.
def event_lines(
    response: http.client.HTTPResponse, sock: socket.socket, ident: int
) -> Iterator[bytes]:
    deadline = time.monotonic() + STEP_SECONDS
    seen: list[bytes] = []
    while (remaining := deadline - time.monotonic()) > 0:
        sock.settimeout(remaining)
        try:
            line = response.readline()
        except TimeoutError as expired:
            raise past_deadline(ident, seen) from expired
        if not line:
            return
        seen.append(line)
        yield line.rstrip(b"\r\n")
    raise past_deadline(ident, seen)


def past_deadline(ident: int, seen: list[bytes]) -> CheckFailedError:
    msg = (
        f"no answer to request {ident} within {STEP_SECONDS} s;"
        f" the stream sent {b''.join(seen)!r} before the deadline"
    )
    return CheckFailedError(msg)


def decode(raw: bytes) -> Any:
    try:
        return json.loads(raw)
    except ValueError as error:
        msg = f"the server sent {raw!r}, which is not JSON"
        raise CheckFailedError(msg) from error


def is_answer(message: Any, ident: int) -> TypeGuard[dict[str, Any]]:
    return isinstance(message, dict) and message.get("id") == ident


if __name__ == "__main__":
    sys.exit(main())
