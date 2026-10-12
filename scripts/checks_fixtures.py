"""Write the checks crate's test fixtures: pair-check inputs and the outcomes Python gives them.

Development-time only. Run it from the repository root:

    uv run python scripts/checks_fixtures.py
    uv run python scripts/checks_fixtures.py --check

recorded.json holds the calls that three unit test modules make to the pair checks, the clone
index, pair_language and in_boundary. A pytest plugin records the inputs of each call. The script
then runs each call again with the unpatched functions, and it keeps a call only when that result
equals the recorded one, so a call that a test patched stays out. seeded.json holds edits of the
characterization fixtures of every tier-1 language and the outcome of each check for each edit.
"""

import argparse
import hashlib
import inspect
import json
from collections.abc import Callable, Collection, Generator, Iterable, Sequence
from dataclasses import dataclass, field
from pathlib import Path
from types import ModuleType
from typing import Any, TypeVar

import pytest

from agentless_mcp.core import complexity, contracts, erosion
from agentless_mcp.core.clones import Block
from agentless_mcp.core.complexity import Unit
from agentless_mcp.core.erosion import CloneIndex, PairFinding, PairOutcome, TextPair
from agentless_mcp.util.errors import UnmeasurableSource

ROOT = Path(__file__).resolve().parents[1]
FIXTURES = ROOT / "crates" / "checks" / "fixtures"
UNIT_TESTS = ROOT / "tests" / "unit"
TEST_TARGETS = (
    str(UNIT_TESTS / "test_erosion.py"),
    str(UNIT_TESTS / "test_clone_delta.py"),
    f"{UNIT_TESTS / 'test_contracts.py'}::TestBoundaryGlobs",
)
LANGUAGE_FIXTURES = ROOT / "tests" / "characterization" / "fixtures"
GLOB_CLASS = "TestBoundaryGlobs"
GRID_TEST = "test_the_matcher_agrees_with_the_regex_it_replaced"

COMPLEXITY_DELTA = erosion.complexity_delta
CONTRACT_DRIFT = erosion.contract_drift
CLONE_DELTA = erosion.clone_delta
CLONE_INDEX = erosion.clone_index
PAIR_LANGUAGE = erosion.pair_language
IN_BOUNDARY = contracts.in_boundary
INDEX_DEFAULTS = inspect.signature(erosion.clone_index).parameters
FILE_BLOCKS = INDEX_DEFAULTS["blocks_of"].default
UNBOUNDED = INDEX_DEFAULTS["in_budget"].default

SEEDED_THRESHOLDS = (complexity.DEFAULT_THRESHOLD, 4)
SEEDED_GLOBS = ("src/**",)
GROWTH_CAP = 10
# One statement per language that one contract rule matches; bash and c have no rules.
HIT_SNIPPETS = {
    "bash": 'value="${options:-}"',
    "c": "int value = options ? options : 0;",
    "cpp": "try { run(); } catch (...) {}",
    "go": "if err != nil { return nil }",
    "java": "int value = options.getOrDefault(key, 0);",
    "javascript": "const value = options ?? {};",
    "lua": "local value = options or {}",
    "python": 'value = options.get("key", {})',
    "ruby": "value = options.fetch(:key, {})",
    "rust": "let value = options.unwrap_or_default();",
    "tsx": "const value = options ?? {};",
    "typescript": "const value = options ?? {};",
}

Json = dict[str, Any]
Result = TypeVar("Result")


class Texts:
    """A table of the source texts the cases share, keyed by a prefix of their sha256."""

    def __init__(self) -> None:
        self.table: dict[str, str] = {}

    def ref(self, text: str | None) -> str | None:
        """Return the key of ``text`` in the table, or None for a side that does not exist."""
        if text is None:
            return None
        key = hashlib.sha256(text.encode("utf-8")).hexdigest()[:16]
        self.table[key] = text
        return key

    def pairs(self, pairs: Iterable[TextPair]) -> list[Json]:
        """Return the JSON form of ``pairs``, with each text as its table key."""
        return [
            {"path": pair.path, "old": self.ref(pair.old), "new": self.ref(pair.new)}
            for pair in pairs
        ]

    def files(self, texts: dict[str, str]) -> dict[str, str | None]:
        """Return the JSON form of a path-to-text map, with each text as its table key."""
        return {path: self.ref(text) for path, text in texts.items()}


@dataclass
class IndexSpec:
    """How to build a clone index again: its texts, or the CloneIndex fields set directly."""

    texts: dict[str, str] = field(default_factory=dict)
    unread: dict[str, str] = field(default_factory=dict)
    budget: list[bool] | None = None
    unlisted: str | None = None
    no_tag_cache: bool = False

    def build(self) -> CloneIndex:
        """Return the index this spec describes, built with the unpatched functions."""
        if self.no_tag_cache:
            return CloneIndex(no_tag_cache=True)
        if self.unlisted is not None:
            return CloneIndex(unlisted=self.unlisted)
        return CLONE_INDEX(self.texts, self.unread, FILE_BLOCKS, replay(self.budget))

    def as_json(self, texts: Texts) -> Json:
        """Return the JSON form of this spec."""
        return {
            "texts": texts.files(self.texts),
            "unread": self.unread,
            "budget": self.budget,
            "unlisted": self.unlisted,
            "no_tag_cache": self.no_tag_cache,
        }


def replay(answers: list[bool] | None) -> Callable[[], bool]:
    """Return a budget that gives ``answers`` in order, or one that never ends for None."""
    if answers is None:
        unbounded: Callable[[], bool] = UNBOUNDED
        return unbounded
    pending = iter(list(answers))
    return lambda: next(pending)


@dataclass
class Call:
    """One recorded call: its kind, the test that made it, its inputs, and what it gave."""

    kind: str
    node: str
    inputs: dict[str, Any]
    result: Json | None = None


class Recorder:
    """A pytest plugin that records each call to the wrapped functions during the tests."""

    def __init__(self) -> None:
        self.node = ""
        self.calls: list[Call] = []
        self.indexes: list[tuple[CloneIndex, IndexSpec | None]] = []
        self.languages: dict[str, str | None] = {}
        self.boundary: list[tuple[str, tuple[str, ...], bool]] = []
        self.grid: dict[str, dict[str, bool]] = {}

    @pytest.hookimpl(hookwrapper=True)
    def pytest_runtest_call(self, item: pytest.Item) -> Generator[None, None, None]:
        """Name the running test, so that each call records the test that made it."""
        self.node = item.nodeid
        yield
        self.node = ""

    def wrappers(self) -> list[tuple[ModuleType, str, Callable[..., Any]]]:
        """Return each module attribute that the recorder replaces, with its replacement."""
        return [
            (erosion, "complexity_delta", self.complexity_delta),
            (erosion, "contract_drift", self.contract_drift),
            (erosion, "clone_delta", self.clone_delta),
            (erosion, "clone_index", self.clone_index),
            (erosion, "pair_language", self.pair_language),
            (contracts, "in_boundary", self.in_boundary),
        ]

    def record(
        self, call: Call, run: Callable[[], Result], form: Callable[[Result], Json]
    ) -> Result:
        """Run ``run`` for ``call``, record its result or the class of its error, and return it."""
        self.calls.append(call)
        try:
            value = run()
        except (ValueError, UnmeasurableSource) as exc:
            call.result = {"raised": type(exc).__name__}
            raise
        call.result = {"outcome": form(value)}
        return value

    def complexity_delta(self, pairs: Sequence[TextPair], threshold: int) -> PairOutcome:
        """Record one complexity_delta call."""
        call = Call("complexity_delta", self.node, {"pairs": tuple(pairs), "threshold": threshold})
        return self.record(call, lambda: COMPLEXITY_DELTA(pairs, threshold), outcome_json)

    def contract_drift(self, pairs: Sequence[TextPair], globs: Sequence[str]) -> PairOutcome:
        """Record one contract_drift call."""
        call = Call("contract_drift", self.node, {"pairs": tuple(pairs), "globs": tuple(globs)})
        return self.record(call, lambda: CONTRACT_DRIFT(pairs, globs), outcome_json)

    def clone_delta(
        self,
        pairs: Sequence[TextPair],
        index: CloneIndex,
        reported: Collection[tuple[str, int]],
    ) -> PairOutcome:
        """Record one clone_delta call, with the spec of the index it reads."""
        inputs = {"pairs": tuple(pairs), "index": self.spec_of(index), "reported": set(reported)}
        call = Call("clone_delta", self.node, inputs)
        return self.record(call, lambda: CLONE_DELTA(pairs, index, reported), outcome_json)

    def clone_index(
        self,
        texts: dict[str, str],
        unread: dict[str, str],
        blocks_of: Callable[..., Any] = FILE_BLOCKS,
        in_budget: Callable[[], bool] = UNBOUNDED,
    ) -> CloneIndex:
        """Record one clone_index call, with each answer its budget gave."""
        answers: list[bool] | None = None if in_budget is UNBOUNDED else []

        def budget() -> bool:
            answer = in_budget()
            if answers is not None:
                answers.append(answer)
            return answer

        spec = IndexSpec(texts=dict(texts), unread=dict(unread), budget=answers)
        call = Call("clone_index", self.node, {"spec": spec, "reader": blocks_of is FILE_BLOCKS})
        index = self.record(call, lambda: CLONE_INDEX(texts, unread, blocks_of, budget), index_json)
        self.indexes.append((index, spec if blocks_of is FILE_BLOCKS else None))
        return index

    def pair_language(self, path: str) -> str | None:
        """Record the language of one path."""
        language = PAIR_LANGUAGE(path)
        self.languages[path] = language
        return language

    def in_boundary(self, path: str, globs: Sequence[str]) -> bool:
        """Record one boundary test that the glob tests make."""
        inside = IN_BOUNDARY(path, globs)
        if GLOB_CLASS not in self.node:
            return inside
        if GRID_TEST in self.node:
            (glob,) = globs
            self.grid.setdefault(glob, {})[path] = inside
        else:
            self.boundary.append((path, tuple(globs), inside))
        return inside

    def spec_of(self, index: CloneIndex) -> IndexSpec | None:
        """Return how to build ``index`` again, or None when the index needs blocks or a reader."""
        for known, spec in self.indexes:
            if known is index:
                return spec
        if index.no_tag_cache:
            return IndexSpec(no_tag_cache=True)
        if not index.files and not index.unindexed:
            return IndexSpec(unlisted=index.unlisted)
        return None


def outcome_json(outcome: PairOutcome) -> Json:
    """Return the JSON form of a pair-check outcome."""
    return {
        "findings": [lint_row(found) for found in outcome.findings],
        "observed": outcome.observed.as_dict(),
    }


def lint_row(found: PairFinding) -> Json:
    """Return one finding as a lint JSON row: its fields, and its detail when it has one."""
    row = found.finding.as_dict()
    if found.detail is not None:
        row["detail"] = found.detail.as_dict()
    return row


def index_json(index: CloneIndex) -> Json:
    """Return the JSON form of the files and gaps of a clone index."""
    files = {
        path: [block_json(block) for block in found.blocks] for path, found in index.files.items()
    }
    return {"files": files, "unindexed": dict(index.unindexed)}


def block_json(block: Block) -> Json:
    """Return the JSON form of one clone block, without its path."""
    return {
        "start_line": block.start_line,
        "end_line": block.end_line,
        "key": block.key,
        "exact": block.exact,
        "function": block.function,
        "tokens": block.tokens,
    }


def rerun(call: Call) -> Json:
    """Run ``call`` again with the unpatched functions and return its result form."""
    try:
        if call.kind == "clone_index":
            return {"outcome": index_json(call.inputs["spec"].build())}
        return {"outcome": outcome_json(run_check(call.kind, call.inputs))}
    except (ValueError, UnmeasurableSource) as exc:
        return {"raised": type(exc).__name__}


def run_check(kind: str, inputs: dict[str, Any]) -> PairOutcome:
    """Run one pair check on ``inputs`` with the unpatched functions."""
    pairs = inputs["pairs"]
    if kind == "complexity_delta":
        return COMPLEXITY_DELTA(pairs, inputs["threshold"])
    if kind == "contract_drift":
        return CONTRACT_DRIFT(pairs, inputs["globs"])
    return CLONE_DELTA(pairs, inputs["index"].build(), frozenset())


def dropped_reason(call: Call) -> str | None:
    """Return why ``call`` cannot be a Rust case, or None when it can."""
    if call.kind == "clone_index" and not call.inputs["reader"]:
        return "a custom block reader"
    if call.kind == "clone_delta" and call.inputs["reported"]:
        return "a non-empty reported set"
    if call.kind == "clone_delta" and call.inputs["index"] is None:
        return "an index from blocks or a custom reader"
    if rerun(call) != call.result:
        return "a patched dependency"
    return None


def case_json(call: Call, texts: Texts) -> Json:
    """Return the JSON form of one call, with the result the unpatched functions give."""
    inputs = call.inputs
    case: Json = {"name": call.node, "check": call.kind}
    if call.kind == "clone_index":
        case["index"] = inputs["spec"].as_json(texts)
    else:
        case["pairs"] = texts.pairs(inputs["pairs"])
    if call.kind == "complexity_delta":
        case["threshold"] = inputs["threshold"]
    if call.kind == "contract_drift":
        case["globs"] = list(inputs["globs"])
    if call.kind == "clone_delta":
        case["index"] = inputs["index"].as_json(texts)
    case["expect"] = rerun(call)
    return case


def unique(cases: Iterable[Json]) -> list[Json]:
    """Return ``cases`` without repeats, where a repeat has the same content under another name."""
    seen: set[str] = set()
    kept: list[Json] = []
    for case in cases:
        content = json.dumps({**case, "name": ""}, sort_keys=True)
        if content not in seen:
            seen.add(content)
            kept.append(case)
    return kept


def record_tests() -> Recorder:
    """Run the three test modules with the recorder installed, and return the recorder."""
    recorder = Recorder()
    replaced = recorder.wrappers()
    originals = [(module, name, getattr(module, name)) for module, name, _ in replaced]
    for module, name, wrapper in replaced:
        setattr(module, name, wrapper)
    try:
        arguments = ["-q", "-p", "no:cacheprovider", "--rootdir", str(ROOT), *TEST_TARGETS]
        status = pytest.main(arguments, plugins=[recorder])
    finally:
        for module, name, original in originals:
            setattr(module, name, original)
    if status != 0:
        message = f"the recorded tests failed with pytest exit status {status}"
        raise SystemExit(message)
    return recorder


def recorded_fixture(recorder: Recorder) -> Json:
    """Return recorded.json: the kept calls, the languages, and the boundary tables."""
    texts = Texts()
    cases: list[Json] = []
    dropped: dict[str, int] = {}
    for call in recorder.calls:
        reason = dropped_reason(call)
        if reason is None:
            cases.append(case_json(call, texts))
        else:
            dropped[reason] = dropped.get(reason, 0) + 1
    kept = unique(cases)
    print(f"recorded {len(recorder.calls)} calls, kept {len(kept)} cases, dropped {dropped}")
    return {
        "texts": dict(sorted(texts.table.items())),
        "cases": kept,
        "pair_language": sorted(recorder.languages.items()),
        "boundary": [[path, list(globs), inside] for path, globs, inside in recorder.boundary],
        "boundary_grid": grid_json(recorder.grid),
    }


def grid_json(grid: dict[str, dict[str, bool]]) -> Json:
    """Return the regex-grid results as the globs, the paths, and one bit row per glob."""
    globs = list(grid)
    paths = list(next(iter(grid.values()), {}))
    if any(list(row) != paths for row in grid.values()):
        message = "the grid test did not try every path with every glob in one order"
        raise SystemExit(message)
    rows = ["".join("1" if row[path] else "0" for path in paths) for row in grid.values()]
    return {"globs": globs, "paths": paths, "expected": rows}


def seeded_fixture() -> Json:
    """Return seeded.json: each edit of each tier-1 fixture file, run through every check."""
    texts = Texts()
    cases: list[Json] = []
    for directory in sorted(LANGUAGE_FIXTURES.glob("complexity_*")):
        for name, pairs, index in directory_runs(directory):
            cases.extend(case_json(call, texts) for call in check_calls(name, pairs, index))
    print(f"seeded {len(cases)} cases")
    return {"texts": dict(sorted(texts.table.items())), "cases": unique(cases)}


def directory_runs(directory: Path) -> list[tuple[str, list[TextPair], IndexSpec]]:
    """Return the runs of one fixture directory: the files together, then each file's edits."""
    sources = {
        f"src/{directory.name}/{file.name}": file.read_bytes().decode("utf-8")
        for file in sorted(directory.iterdir())
        if file.is_file()
    }
    index = IndexSpec(texts=sources)
    runs = [
        (f"{directory.name}:created", [TextPair(p, None, t) for p, t in sources.items()], index),
        (f"{directory.name}:deleted", [TextPair(p, t, None) for p, t in sources.items()], index),
    ]
    for path, text in sources.items():
        language = PAIR_LANGUAGE(path)
        if language is None:
            message = f"{path} is not a tier-1 source file"
            raise SystemExit(message)
        runs.extend((f"{path}:{name}", pairs, index) for name, pairs in edits(path, text, language))
    return runs


def check_calls(name: str, pairs: list[TextPair], index: IndexSpec) -> list[Call]:
    """Return one call of each check on ``pairs``, and one complexity call per threshold."""
    calls = [
        Call("complexity_delta", name, {"pairs": pairs, "threshold": threshold})
        for threshold in SEEDED_THRESHOLDS
    ]
    calls.append(Call("contract_drift", name, {"pairs": pairs, "globs": SEEDED_GLOBS}))
    calls.append(Call("clone_delta", name, {"pairs": pairs, "index": index, "reported": set()}))
    return calls


def edits(path: str, text: str, language: str) -> list[tuple[str, list[TextPair]]]:
    """Return the seeded edits of one fixture file, each as the pairs of one run."""
    found = [
        ("unchanged", [TextPair(path, text, text)]),
        ("created", [TextPair(path, None, text)]),
        ("deleted", [TextPair(path, text, None)]),
    ]
    unit = largest_unit(text, path, language)
    if unit is None:
        return found
    rows = text.split("\n")
    first, last = unit.start_line - 1, unit.end_line
    return [
        *found,
        ("grown", [TextPair(path, text, grown(rows, unit, language))]),
        ("renamed", [TextPair(path, text, renamed(rows, unit))]),
        ("copied", [TextPair(path, text, copied(rows, unit))]),
        ("hit", [TextPair(path, text, with_hit(rows, unit, language))]),
        (
            "moved",
            [
                TextPair(path, text, joined(rows[:first], rows[last:])),
                TextPair(moved_path(path), None, text),
            ],
        ),
    ]


def largest_unit(text: str, path: str, language: str) -> Unit | None:
    """Return the first unit with the highest complexity, or None for a text it cannot measure."""
    try:
        units = complexity.measure(text, path, language)
    except UnmeasurableSource:
        return None
    return max(units, key=lambda unit: unit.cc, default=None)


def joined(*parts: list[str]) -> str:
    """Return the rows of ``parts`` in order, joined with newlines."""
    return "\n".join(row for part in parts for row in part)


def grown(rows: list[str], unit: Unit, language: str) -> str:
    """Return the text with the unit's body repeated until its complexity passes the threshold."""
    first, last = unit.start_line - 1, unit.end_line
    body_end = last if language == "python" else last - 1
    body = rows[first + 1 : body_end]
    copies = min(GROWTH_CAP, 1 + complexity.DEFAULT_THRESHOLD // max(unit.cc - 1, 1))
    return joined(rows[:body_end], body * copies, rows[body_end:])


def renamed(rows: list[str], unit: Unit) -> str:
    """Return the text with the unit's own name changed on its first line."""
    first = unit.start_line - 1
    name = unit.qualname.rsplit(".", 1)[-1]
    line = rows[first].replace(name, f"{name}_renamed", 1)
    return joined(rows[:first], [line], rows[first + 1 :])


def copied(rows: list[str], unit: Unit) -> str:
    """Return the text with a copy of the unit's lines after the unit."""
    first, last = unit.start_line - 1, unit.end_line
    return joined(rows[:last], [""], rows[first:last], rows[last:])


def with_hit(rows: list[str], unit: Unit, language: str) -> str:
    """Return the text with the language's contract-hit statement after the unit's first line."""
    first = unit.start_line - 1
    following = rows[first + 1] if first + 1 < len(rows) else ""
    indent = following[: len(following) - len(following.lstrip())]
    return joined(rows[: first + 1], [indent + HIT_SNIPPETS[language]], rows[first + 1 :])


def moved_path(path: str) -> str:
    """Return the path of the new file that a moved unit goes to."""
    head, _, name = path.rpartition("/")
    return f"{head}/moved_{name}"


def dumped(data: Json) -> str:
    """Return ``data`` as JSON with sorted keys and one line per list item or table entry."""
    members = [f"{compact(key)}: {member(value)}" for key, value in sorted(data.items())]
    return "{\n" + ",\n".join(members) + "\n}\n"


def member(value: Any) -> str:
    """Return one top-level value, with each list item or table entry on its own line."""
    if isinstance(value, list) and value:
        return "[\n" + ",\n".join(compact(item) for item in value) + "\n]"
    if isinstance(value, dict) and value:
        rows = [f"{compact(key)}: {compact(item)}" for key, item in sorted(value.items())]
        return "{\n" + ",\n".join(rows) + "\n}"
    return compact(value)


def compact(value: Any) -> str:
    """Return ``value`` as one line of JSON with sorted keys."""
    return json.dumps(value, sort_keys=True, ensure_ascii=False, separators=(",", ":"))


def fixture_texts() -> dict[str, str]:
    """Return the text of each fixture file, by file name."""
    recorder = record_tests()
    return {
        "recorded.json": dumped(recorded_fixture(recorder)),
        "seeded.json": dumped(seeded_fixture()),
    }


def main() -> None:
    """Write the fixtures, or with --check, fail when a committed fixture is stale."""
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--check", action="store_true", help="compare, and write nothing")
    arguments = parser.parse_args()
    wanted = fixture_texts()
    if arguments.check:
        stale = [
            name
            for name, text in wanted.items()
            if not (FIXTURES / name).is_file() or (FIXTURES / name).read_text("utf-8") != text
        ]
        if stale:
            message = f"stale fixtures in {FIXTURES}: {', '.join(stale)}"
            raise SystemExit(message)
        return
    FIXTURES.mkdir(parents=True, exist_ok=True)
    for name, text in wanted.items():
        (FIXTURES / name).write_text(text, encoding="utf-8")


if __name__ == "__main__":
    main()
