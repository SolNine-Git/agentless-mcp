import dataclasses
import hashlib
import json
import sys
from collections.abc import Mapping
from pathlib import Path

import pytest

from agentless_mcp.core import clones, complexity, extractor, fileparse, grammars, normalize
from agentless_mcp.core.extractor import TreeSitterExtractor
from agentless_mcp.core.fileparse import (
    METRICS_ALGORITHM,
    Unmeasured,
    metrics_stamp,
    parse_file,
    parse_metrics,
)
from agentless_mcp.prompts import MESSAGES
from agentless_mcp.util.errors import UnmeasurableSource

FIXTURES = Path(__file__).parent.parent / "characterization" / "fixtures"

# The digests of what METRICS_ALGORITHM promises. Change a pin only together with a bump.
TABLES_PIN = {
    1: "231908e0a6e8a05df3210fe1dda800a731cec8038c9645b192047068ff618c40",
}
OUTPUTS_PIN = {
    (1, "1.14.3"): (
        "a99b1b48fe7d4313659c5524de40326303de46b93b23120f1001c77f6504ffa9",
        "b77ffc9d5ef9b6fd2c0b36d45ba831574486c8c71cefc5e6eaf9e59d31d74f2d",
    ),
}
BUMP = (
    "complexity or clone results changed: bump fileparse.METRICS_ALGORITHM and pin the new "
    "digests here, so that every cached metric row is recomputed"
)
NEW_PACK = (
    "no output pin for this grammar pack: pin its digests under the current "
    "METRICS_ALGORITHM (a pack change already expires every cached row)"
)
METRIC_MESSAGES = (
    "complexity_language_unsupported",
    "complexity_does_not_parse",
    "complexity_tokenizer_rejects",
    "clone_language_unsupported",
    "clone_does_not_parse",
    "clone_tokenizer_rejects",
)


def corpus():
    paths = [
        *sorted(FIXTURES.glob("complexity_*/*")),
        *sorted((FIXTURES / "tier1").iterdir()),
        *sorted((FIXTURES / "tier2").iterdir()),
    ]
    for path in paths:
        language = TreeSitterExtractor.SUPPORTED_EXTENSIONS.get(path.suffix)
        if path.is_file() and language is not None:
            yield pytest.param(path, language, id=f"{path.parent.name}/{path.name}")


def metric_fixtures(*, python):
    for path in sorted(FIXTURES.glob("complexity_*/*")):
        language = TreeSitterExtractor.SUPPORTED_EXTENSIONS.get(path.suffix)
        if path.is_file() and language in complexity.RULES and (language == "python") is python:
            yield path, language


def outcome(function, *arguments):
    try:
        return function(*arguments)
    except UnmeasurableSource as exc:
        return Unmeasured(str(exc))


def canonical(value):
    if dataclasses.is_dataclass(value) and not isinstance(value, type):
        fields = dataclasses.fields(value)
        return {entry.name: canonical(getattr(value, entry.name)) for entry in fields}
    if isinstance(value, Mapping):
        return {str(key): canonical(item) for key, item in sorted(value.items())}
    return canonical_value(value)


def canonical_value(value):
    if isinstance(value, (set, frozenset)):
        return sorted(canonical(item) for item in value)
    if isinstance(value, (list, tuple)):
        return [canonical(item) for item in value]
    if callable(value):
        return f"{value.__module__}.{value.__qualname__}"
    return value


def digest(value):
    text = json.dumps(canonical(value), sort_keys=True, separators=(",", ":"))
    return hashlib.sha256(text.encode("utf-8")).hexdigest()


def tables():
    return {
        "rules": complexity.RULES,
        "candidates": clones.CANDIDATES,
        "min_sloc": clones.MIN_SLOC,
        "identifiers": normalize.CLONE_IDENTIFIERS,
        "literals": normalize.CLONE_LITERALS,
        "statement_drops": normalize._STATEMENT_DROPS,
        "statement_parents": normalize.PYTHON_STATEMENT_PARENTS,
        "comments": extractor.COMMENT_NODE_TYPES,
        "indent_blocks": extractor.INDENT_BLOCK_NODE_TYPES,
        "messages": [getattr(MESSAGES, name) for name in METRIC_MESSAGES],
    }


def outputs(*, python):
    rows = []
    for path, language in metric_fixtures(python=python):
        text = path.read_text(encoding="utf-8")
        name = f"{path.parent.name}/{path.name}"
        rows.append(
            [
                name,
                outcome(complexity.measure, text, name, language),
                outcome(clones.file_blocks, text, name, language),
            ]
        )
    return rows


class TestOneParse:
    @pytest.mark.parametrize(("path", "language"), list(corpus()))
    def test_one_parse_gives_what_the_text_forms_give(self, extractor, path, language):
        text = path.read_text(encoding="utf-8")

        parsed = parse_file(extractor, text, language, path.name)

        assert parsed.facts == extractor.extract_facts(text, language, path.name)
        if language not in complexity.RULES:
            assert parsed.metrics is None
            return
        assert parsed.metrics.stamp == metrics_stamp(language)
        assert parsed.metrics.language == language
        assert parsed.metrics.units == outcome(complexity.measure, text, path.name, language)
        assert parsed.metrics.blocks == outcome(clones.file_blocks, text, path.name, language)

    def test_a_file_is_parsed_once(self, extractor, monkeypatch):
        calls = []
        real = grammars.get_parser

        def counted(language):
            calls.append(language)
            return real(language)

        monkeypatch.setattr(grammars, "get_parser", counted)
        source = "def f(x):\n    if x:\n        return 1\n    return 2\n"

        parse_file(extractor, source, "python", "a.py")

        assert calls == ["python"]

    @pytest.mark.parametrize(
        ("source", "reads"),
        [
            pytest.param("def f(x):\n    if x:\n        return 1\n    return 2\n", 1, id="once"),
            pytest.param("def f():\n    if x:\n        pass\n  pass\n", 2, id="rejected"),
        ],
    )
    def test_the_sloc_lines_are_read_once_unless_the_tokenizer_rejects(
        self, extractor, monkeypatch, source, reads
    ):
        calls = []
        real = complexity.sloc_line_numbers

        def counted(root, text, language):
            calls.append(language)
            return real(root, text, language)

        monkeypatch.setattr(complexity, "sloc_line_numbers", counted)

        parse_file(extractor, source, "python", "a.py")

        assert len(calls) == reads

    def test_a_file_that_does_not_parse_keeps_its_facts_and_both_reasons(self, extractor):
        source = "def good():\n    return 1\n\ndef bad(:\n"

        parsed = parse_file(extractor, source, "python", "a.py")

        assert [symbol.name for symbol in parsed.facts.symbols][:1] == ["good"]
        assert isinstance(parsed.metrics.units, Unmeasured)
        assert isinstance(parsed.metrics.blocks, Unmeasured)
        assert "does not parse" in parsed.metrics.units.reason
        assert parsed.metrics.units.reason != parsed.metrics.blocks.reason

    def test_a_tokenizer_rejection_gives_both_tokenizer_reasons(self, extractor):
        source = "def f():\n    if x:\n        pass\n  pass\n"

        metrics = parse_file(extractor, source, "python", "a.py").metrics

        assert "tokenizer rejects" in metrics.units.reason
        assert "tokenizer rejects" in metrics.blocks.reason

    def test_a_language_without_metrics_has_none(self, extractor):
        parsed = parse_file(extractor, '{"a": 1}\n', "json", "a.json")

        assert parsed.metrics is None


def text_forms(text, language, path):
    units = complexity.measure(text, path, language)
    return units, clones.file_blocks(text, path, language)


def counting(monkeypatch, module, name):
    """Replace ``module.name`` with a wrapper that records the first argument of every call."""
    calls = []
    real = getattr(module, name)

    def counted(*arguments):
        calls.append(arguments[0])
        return real(*arguments)

    monkeypatch.setattr(module, name, counted)
    return calls


class TestTheMetricsParse:
    @pytest.mark.parametrize(("path", "language"), list(corpus()))
    def test_one_parse_gives_or_raises_what_the_text_forms_do(self, path, language):
        text = path.read_text(encoding="utf-8")

        assert outcome(parse_metrics, text, language, path.name) == outcome(
            text_forms, text, language, path.name
        )

    def test_a_file_is_parsed_once_and_its_sloc_lines_read_once(self, monkeypatch):
        parses = counting(monkeypatch, grammars, "get_parser")
        sloc = counting(monkeypatch, complexity, "sloc_line_numbers")
        source = "def f(x):\n    if x:\n        return 1\n    return 2\n"

        units, blocks = parse_metrics(source, "python", "a.py")

        assert parses == ["python"]
        assert len(sloc) == 1
        assert [unit.qualname for unit in units] == ["f"]
        assert blocks.sloc_lines == (1, 2, 3, 4)

    def test_the_units_failure_is_raised_first(self):
        source = "def good():\n    return 1\n\ndef bad(:\n"

        with pytest.raises(UnmeasurableSource) as raised:
            parse_metrics(source, "python", "a.py")

        assert outcome(complexity.measure, source, "a.py", "python") == Unmeasured(
            str(raised.value)
        )

    def test_a_language_without_metrics_raises_the_complexity_refusal(self):
        with pytest.raises(UnmeasurableSource) as raised:
            parse_metrics('{"a": 1}\n', "json", "a.json")

        assert outcome(complexity.measure, '{"a": 1}\n', "a.json", "json") == Unmeasured(
            str(raised.value)
        )


class TestTheStamp:
    def test_a_language_without_metrics_has_no_stamp(self):
        assert metrics_stamp("json") is None

    @pytest.mark.parametrize("language", sorted(complexity.RULES))
    def test_a_tier1_stamp_names_the_algorithm_and_the_interpreter(self, language):
        major, minor = sys.version_info[:2]
        assert metrics_stamp(language) == f"{METRICS_ALGORITHM}/py{major}.{minor}"

    def test_the_stamp_follows_the_algorithm_version(self, monkeypatch):
        monkeypatch.setattr(fileparse, "METRICS_ALGORITHM", METRICS_ALGORITHM + 1)
        assert metrics_stamp("python").startswith(f"{METRICS_ALGORITHM + 1}/")


class TestTheAlgorithmVersion:
    def test_the_tables_and_messages_match_the_pin(self):
        assert digest(tables()) == TABLES_PIN.get(METRICS_ALGORITHM), BUMP

    def test_the_fixture_results_match_the_pin(self):
        pin = OUTPUTS_PIN.get((METRICS_ALGORITHM, grammars.pack_version()))
        assert pin is not None, NEW_PACK
        assert digest(outputs(python=False)) == pin[0], BUMP
        # Python SLOC follows the running tokenizer, and parity is defined from 3.12 on.
        if sys.version_info >= (3, 12):
            assert digest(outputs(python=True)) == pin[1], BUMP

    def test_the_pin_covers_the_cached_token_count(self):
        (path, language), *_ = metric_fixtures(python=False)
        text = path.read_text(encoding="utf-8")
        indexed = clones.file_blocks(text, path.name, language)
        bumped = dataclasses.replace(
            indexed,
            blocks=tuple(
                dataclasses.replace(block, tokens=block.tokens + 1) for block in indexed.blocks
            ),
        )

        assert indexed.blocks
        assert digest(bumped) != digest(indexed)
