"""Write the prompts crate's test fixtures: each template with its test arguments and its text.

Development-time only. Run it from the repository root:

    uv run python scripts/prompts_fixtures.py

render.json holds, for each template, the arguments that tests/unit/test_prompts.py gives it and
the text that Python's str.format makes from them. contract_rules.json holds the contract rule
records in their load order. With --check, the script compares its output with the committed
fixtures and writes nothing.
"""

import ast
import dataclasses
import json
from pathlib import Path

from fixture_files import write_or_check_files

from agentless_mcp.prompts import CONTRACT_RULES, ENVELOPE, MESSAGES

ROOT = Path(__file__).resolve().parents[1]
TESTS = ROOT / "tests" / "unit" / "test_prompts.py"
FIXTURES = ROOT / "crates" / "prompts" / "fixtures"
TABLES = {"envelope": ("ENVELOPE_ARGUMENTS", ENVELOPE), "messages": ("MESSAGE_ARGUMENTS", MESSAGES)}

Arguments = dict[str, dict[str, object]]


def argument_tables() -> dict[str, Arguments]:
    """Return the test file's argument tables, read as literals, so the test module is not run."""
    wanted = {table for table, _ in TABLES.values()}
    tables: dict[str, Arguments] = {}
    for node in ast.parse(TESTS.read_text(encoding="utf-8")).body:
        if isinstance(node, ast.Assign) and len(node.targets) == 1:
            target = node.targets[0]
            if isinstance(target, ast.Name) and target.id in wanted:
                tables[target.id] = ast.literal_eval(node.value)
    missing = sorted(wanted - set(tables))
    if missing:
        message = f"{TESTS} does not assign {missing}"
        raise SystemExit(message)
    return tables


def render_cases(tables: dict[str, Arguments]) -> dict[str, dict[str, object]]:
    """Return each template's arguments and the text that str.format makes from them."""
    cases: dict[str, dict[str, object]] = {}
    for file, (table, record) in TABLES.items():
        arguments = tables[table]
        differ = sorted(set(vars(record)) ^ set(arguments))
        if differ:
            message = f"{table} does not match the {file} templates: {differ}"
            raise SystemExit(message)
        cases[file] = {
            key: {"arguments": values, "text": getattr(record, key).format(**values)}
            for key, values in arguments.items()
        }
    return cases


def dumped(data: object) -> bytes:
    """Return ``data`` as sorted, indented JSON with a final newline."""
    return (json.dumps(data, indent=2, sort_keys=True, ensure_ascii=False) + "\n").encode("utf-8")


def contents() -> dict[Path, bytes]:
    """Return the bytes of render.json and contract_rules.json, by path."""
    return {
        FIXTURES / "render.json": dumped(render_cases(argument_tables())),
        FIXTURES / "contract_rules.json": dumped(
            [dataclasses.asdict(rule) for rule in CONTRACT_RULES]
        ),
    }


def main() -> int:
    """Write the fixtures, or compare them with --check, and return the exit status."""
    return write_or_check_files(__doc__.splitlines()[0], FIXTURES, contents)


if __name__ == "__main__":
    raise SystemExit(main())
