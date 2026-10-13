"""Write the render crate's read-envelope fixtures: the envelope parts that the read answers use.

Development-time only. Run it from the repository root, with scratch cache directories:

    XDG_CACHE_HOME=<scratch> TREE_SITTER_LANGUAGE_PACK_CACHE_DIR=<scratch> \
        .venv/bin/python scripts/envelope_fixtures.py

The sections cover the service truncation note, the untrimmable JSON answer, a list at a dotted
key, the CLI "truncation" key, the stderr receipt with a summary, and a counter that is not chars/4.
Every payload keeps its keys in code point order, so a serde_json value in the Rust tests writes
them in the same order. With --check, the script compares its output with the committed fixtures
and writes nothing.
"""

from pathlib import Path
from typing import Any

from fixture_files import Fixture, refuse_real_caches, write_or_check
from render_fixtures import (
    CONFIG_PATH,
    FORGED,
    context,
    lines_body,
    receipt,
    receipts,
    warning_texts,
)

from agentless_mcp.adapters.cli import main
from agentless_mcp.application import envelope
from agentless_mcp.util.tokens import Chars4Counter, TokenCounter

FIXTURES = Path(__file__).resolve().parents[1] / "crates" / "render" / "fixtures" / "read"
CHARS4 = Chars4Counter()
CEILING = envelope.DEFAULT_MAX_TOKENS
E_ACUTE = chr(0xE9)
EMOJI = chr(0x1F600)
DOTTED = "bodies.symbols"

Truncation = list[Any] | None


class DistinctCounter:
    """Counts the distinct code points of a text: not additive, so each count site shows."""

    def count(self, text: str) -> int:
        """Return the number of distinct code points in ``text``."""
        return len(set(text))


DISTINCT = DistinctCounter()


def require_sorted(value: object) -> object:
    if isinstance(value, dict):
        keys = list(value)
        if keys != sorted(keys):
            refusal = f"payload keys out of code point order: {keys}"
            raise SystemExit(refusal)
        for item in value.values():
            require_sorted(item)
    elif isinstance(value, list):
        for item in value:
            require_sorted(item)
    return value


def cut(truncation: Truncation) -> envelope.Truncation | None:
    return None if truncation is None else envelope.Truncation(*truncation)


def symbol(number: int, width: int = 1) -> dict[str, Any]:
    return {
        "body": f"def f{number}():\n" + "    return 1\n" * width,
        "name": f"f{number}",
        "score": number / 7,
    }


def body_map(count: int, width: int = 1, files: int = 3) -> dict[str, Any]:
    return require_sorted(
        {
            "bodies": {
                "budget": 4000,
                "symbols": [symbol(number, width) for number in range(count)],
            },
            "files": [{"path": f"src/m{number}.py", "rank": 0.5} for number in range(files)],
        }
    )


def tree_view(entries: int) -> dict[str, Any]:
    return require_sorted(
        {
            "depth": 2,
            "entries": [f"src/{E_ACUTE}{number}.py" for number in range(entries)],
            "note": f"tree of {EMOJI}",
            "ratio": -0.0,
            "root": "/srv/app",
            "skipped": None,
        }
    )


def service_cases() -> list[object]:
    truncations: list[Truncation] = [None, [3, 10, "symbols"], [10, 10, "symbols"]]
    truncations += [[12, 10, "files"], [0, 5, "matches"]]
    rows: list[list[Any]] = [
        [receipt(), body, ceiling, truncation]
        for body in ("body\n", lines_body(40), "")
        for ceiling in (CEILING, 120, 0)
        for truncation in truncations
    ]
    rows.append([receipt(config=["{}", CONFIG_PATH], warnings=warning_texts(10)), "x\n", 200, None])
    return [[*row, wrap(CHARS4, *row)] for row in rows]


def wrap(
    counter: TokenCounter, row: dict[str, Any], body: str, ceiling: int, truncation: Truncation
) -> str:
    return envelope.wrap(
        context(row), body, counter=counter, max_tokens=ceiling, truncation=cut(truncation)
    )


def wrap_json(
    counter: TokenCounter,
    row: dict[str, Any],
    payload: dict[str, Any],
    items_key: str | None,
    ceiling: int,
) -> str:
    return envelope.wrap_json(
        context(row), payload, counter=counter, max_tokens=ceiling, items_key=items_key
    )


def untrimmable_cases() -> list[object]:
    payloads = [tree_view(0), tree_view(3), tree_view(120)]
    rows = [
        [receipt(), payload, ceiling]
        for payload in payloads
        for ceiling in (CEILING, 200, 50, 1, 0)
    ]
    rows.append([receipt(warnings=["x" * 9000, *warning_texts(7)]), tree_view(3), 400])
    return [
        [row, payload, ceiling, wrap_json(CHARS4, row, payload, None, ceiling)]
        for row, payload, ceiling in rows
    ]


def nested_cases() -> list[object]:
    cases: list[list[Any]] = [
        [body_map(3), DOTTED, CEILING],
        [body_map(20, width=4), DOTTED, 600],
        [body_map(20, width=4), DOTTED, 100],
        [body_map(2, width=400), DOTTED, 300],
        [body_map(0, files=200), DOTTED, 300],
        [body_map(20, width=4), "files", 600],
    ]
    return [
        [receipt(), payload, key, ceiling, wrap_json(CHARS4, receipt(), payload, key, ceiling)]
        for payload, key, ceiling in cases
    ]


def with_truncation(
    payload: dict[str, Any], key: str | None, truncation: Truncation
) -> dict[str, Any]:
    answer = main._Answer(text="", payload=payload, items_key=key, truncation=cut(truncation))
    return main._with_truncation(answer)


def truncation_key_cases() -> list[object]:
    cases: list[list[Any]] = [
        [body_map(3), DOTTED, None, CEILING],
        [body_map(3), DOTTED, [3, 10, "symbols"], CEILING],
        [body_map(3), DOTTED, [10, 10, "symbols"], CEILING],
        [body_map(20, width=4), DOTTED, [20, 90, "symbols"], 600],
        [body_map(20, width=4), "files", [3, 10, "symbols"], 600],
        [tree_view(3), None, [3, 10, "entries"], CEILING],
        [tree_view(120), None, [120, 900, "entries"], 50],
    ]
    rows = []
    for payload, key, truncation, ceiling in cases:
        document = with_truncation(payload, key, truncation)
        expected = wrap_json(CHARS4, receipt(), document, key, ceiling)
        rows.append([receipt(), payload, key, truncation, ceiling, expected])
    return rows


def receipt_line_cases() -> list[object]:
    summaries = ["HTML graph of 3 modules and 2 edges", FORGED, "", "a\r\nb"]
    rows = [[row, summary] for row in receipts() for summary in summaries[:1]]
    rows += [[receipt(), summary] for summary in summaries[1:]]
    return [
        [row, summary, envelope.receipt_lines(context(row), summary=summary)]
        for row, summary in rows
    ]


def distinct_text_cases(truncation: Truncation) -> list[object]:
    warned = receipt(config=["{}", CONFIG_PATH], warnings=warning_texts(10))
    rows: list[list[Any]] = [
        [row, lines_body(40), ceiling, truncation]
        for row in (receipt(), warned, receipt(warnings=["x" * 9000, *warning_texts(7)]))
        for ceiling in (CEILING, 120, 60, 30)
    ]
    return [[*row, wrap(DISTINCT, *row)] for row in rows]


def distinct_json_cases(payload: dict[str, Any], key: str | None) -> list[object]:
    warned = receipt(config=["{}", CONFIG_PATH], warnings=warning_texts(10))
    cases: list[list[Any]] = [
        [row, payload, key, ceiling]
        for row in (receipt(), warned)
        for ceiling in (CEILING, 70, 55, 40)
    ]
    return [[*case, wrap_json(DISTINCT, *case)] for case in cases]


# The service note, the untrimmable answer and the "truncation" key land with their first caller in
# the render crate, so the sections that need them stand apart from the ones tested now.
def envelope_fixture() -> Fixture:
    return {
        "nested": nested_cases(),
        "receipt_lines": receipt_line_cases(),
        "distinct_text": distinct_text_cases(None),
        "distinct_json": distinct_json_cases(body_map(12, width=3), DOTTED),
        "service": service_cases(),
        "untrimmable": untrimmable_cases(),
        "truncation_key": truncation_key_cases(),
        "distinct_service": distinct_text_cases([3, 10, "symbols"]),
        "distinct_untrimmable": distinct_json_cases(tree_view(20), None),
    }


BUILDERS = {"envelope": envelope_fixture}


if __name__ == "__main__":
    refuse_real_caches()
    raise SystemExit(write_or_check((__doc__ or "").splitlines()[0], FIXTURES, BUILDERS))
