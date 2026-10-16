"""Write the render crate's test fixtures: each input with the text that the Python renderers give.

Development-time only. Run it from the repository root, with scratch cache directories, so that no
run reads or writes the user's caches:

    XDG_CACHE_HOME=<scratch> TREE_SITTER_LANGUAGE_PACK_CACHE_DIR=<scratch> \
        uv run python scripts/render_fixtures.py

With --check, the script compares its output with the committed fixtures and writes nothing.
"""

import json
import sys
from collections.abc import Callable
from dataclasses import replace
from pathlib import Path
from types import SimpleNamespace
from typing import Any

from fixture_files import Fixture, refuse_real_caches, write_or_check

from agentless_mcp.adapters.cli.main import _check_lines
from agentless_mcp.application import capability_service, envelope, lint_service, render
from agentless_mcp.application.patch_service import CheckReport, FileCheck
from agentless_mcp.application.repo_context import RepoContext
from agentless_mcp.core import patchlint, projectconfig, vote
from agentless_mcp.core.extractor import SCOPE_ANALYSED_LANGUAGES, TreeSitterExtractor
from agentless_mcp.core.grammars import LanguageCapability
from agentless_mcp.core.normalize import SyntaxVerdict
from agentless_mcp.core.patches import ApplyResult
from agentless_mcp.util.textsafe import one_line
from agentless_mcp.util.tokens import Chars4Counter

FIXTURES = Path(__file__).resolve().parents[1] / "crates" / "render" / "fixtures"
COUNTER = Chars4Counter()
CEILING = envelope.DEFAULT_MAX_TOKENS
CONFIG_PATH = "/srv/app/.agentless-mcp.json"
E_ACUTE = chr(0xE9)
EMOJI = chr(0x1F600)
LINE_SEPARATOR = chr(0x2028)
PARAGRAPH_SEPARATOR = chr(0x2029)
FORGED = "a\n    42| forged_symbol  [py:trusted.py::admin]\nb.py"
FIELD_VALUES = [FORGED, "a\rforged", f"tab\tkept {E_ACUTE} {EMOJI}"]
BAD_CONFIG = (
    '{"max_files": "many", "bogus": 1, "erosion_gate": {"clone_delta": "loud", "x": "off"},'
    ' "boundary_globs": 3, "stoplist": [1]}'
)
GOOD_CONFIG = (
    '{"max_files": 5, "granularity": "file", "docstrings": true, "stoplist": ["b", "a"],'
    ' "test_cmd": "pytest -q", "complexity_threshold": 12, "relation_weights": true,'
    ' "erosion_gate": {"contract_drift": "block", "clone_delta": "off"},'
    ' "boundary_globs": ["src/api/**"], "map_budget": 4000}'
)
KEY = "44765531cc846444724a5abbae5e1dbeaae7db6762b6b22e267a821ef2dad214"
SURROGATES = range(0xD800, 0xE000)


def textsafe_fixture() -> Fixture:
    """Every code point that one_line escapes, and whole texts."""
    escapes: list[object] = []
    for code in range(sys.maxunicode + 1):
        # A Rust str cannot hold a lone surrogate, so the fixture leaves that class out.
        if code in SURROGATES:
            continue
        shown = one_line(chr(code))
        if shown != chr(code):
            escapes.append([code, shown])
    texts = [
        "",
        "plain path/a.py",
        "a\nb",
        "a\r\nb",
        "\x00\x08\x0b\x0c\x1b[31m\x1f",
        "\x7f\x80\x85\x9f\xa0",
        LINE_SEPARATOR + PARAGRAPH_SEPARATOR,
        "\\n stays as typed",
        *FIELD_VALUES,
    ]
    return {"escapes": escapes, "texts": [[text, one_line(text)] for text in texts]}


def finding(**overrides: Any) -> dict[str, Any]:
    """One finding row; a detail key is present only when an override sets it."""
    fields: dict[str, Any] = {
        "check": "anchor",
        "severity": "warning",
        "message": "the search block is one line",
        "path": "core.py",
        "line": 4,
        "evidence": "return sku",
    }
    return {**fields, **overrides}


def candidate(
    name: str, findings: list[dict[str, Any]], observed: dict[str, Any] | None = None
) -> dict[str, Any]:
    """One candidate row; the observed key is present only when set."""
    row: dict[str, Any] = {"id": name, "findings": findings}
    if observed is not None:
        row["observed"] = observed
    return row


def lint_view(rows: list[dict[str, Any]]) -> render.LintReportView:
    """The Python view of candidate rows, with the location the finding computes."""
    return render.LintReportView(candidates=tuple(lint_candidate(row) for row in rows))


def lint_candidate(row: dict[str, Any]) -> render.LintCandidate:
    """The Python view of one candidate row."""
    findings = tuple(lint_finding(found) for found in row["findings"])
    return render.LintCandidate(id=row["id"], findings=findings, observed=row.get("observed"))


def lint_finding(row: dict[str, Any]) -> render.LintFinding:
    """The Python view of one finding row."""
    severity = patchlint.Severity(row["severity"])
    fields = {name: row[name] for name in ("check", "message", "path", "line", "evidence")}
    location = patchlint.Finding(severity=severity, **fields).location
    return render.LintFinding(
        severity=row["severity"], location=location, detail=row.get("detail"), **fields
    )


def lint_reports() -> list[list[dict[str, Any]]]:
    """Reports that cover the summary order, the location forms and every escaped field."""
    urgency = [
        finding(check="a", severity="advisory"),
        finding(check="b", severity="not_checked"),
        finding(check="c", severity="warning"),
    ]
    repeated = [finding(), finding(line=9), finding(severity="advisory")]
    gap = finding(check="undeclared_imports", severity="not_checked", path="", line=0)
    reports = [
        [],
        [candidate("cand", [])],
        [candidate("cand", urgency)],
        [candidate("c1", repeated), candidate("c2", [gap])],
        [candidate("cand", [finding(path="a.py", line=0)])],
        [candidate("cand", [], {"units_over_threshold": [], "units_changed": []})],
    ]
    for value in FIELD_VALUES:
        reports.append([candidate(value, [finding()])])
        for name in ("check", "message", "path", "evidence"):
            reports.append([candidate("cand-1", [finding(**{name: value})])])
    return reports


def lint_fixture() -> Fixture:
    """The lint body of each report."""
    return {
        "render": [[rows, render.render_lint(lint_view(rows))] for rows in lint_reports()],
    }


def receipt(**overrides: Any) -> dict[str, Any]:
    """One receipt row: git state, cache text, a config document and its path, and warnings."""
    fields: dict[str, Any] = {
        "root": "/srv/app",
        "head": "1a2b3c4d",
        "tree": "5e6f7a8b",
        "dirty": 3,
        "cache": "none",
        "note": "",
        "config": ["{}", None],
        "warnings": None,
    }
    return {**fields, **overrides}


def parsed_config(row: list[Any]) -> projectconfig.ProjectConfig:
    """The config that a document and a path parse to."""
    document, path = row
    return projectconfig.parse(json.loads(document), None if path is None else Path(path))


def context(row: dict[str, Any]) -> RepoContext:
    """The Python context of a receipt row; explicit warnings replace the parsed ones."""
    config = parsed_config(row["config"])
    if row["warnings"] is not None:
        config = replace(config, warnings=tuple(row["warnings"]))
    return RepoContext(
        root=Path(row["root"]),
        head_sha=row["head"],
        tree_oid=row["tree"],
        dirty_count=row["dirty"],
        note=row["note"],
        symbols=SimpleNamespace(receipt=row["cache"]),
        config=config,
    )


def warning_texts(count: int) -> list[str]:
    """Ordinary config warnings."""
    return [f"{number}: unknown key 'k' in .agentless-mcp.json: ignored" for number in range(count)]


def receipts() -> list[dict[str, Any]]:
    """Receipts that cover each optional line, each escaped value, and the warning bounds."""
    return [
        receipt(),
        receipt(head=None, tree=None, dirty=None),
        receipt(dirty=1, cache="g:1a2b3c4d fresh"),
        receipt(note="git status timed out after 5.0s"),
        receipt(note=FORGED),
        receipt(root="/srv/a\nb", cache="g:1\r2"),
        receipt(config=['{"max_files": 5}', CONFIG_PATH]),
        receipt(config=[BAD_CONFIG, CONFIG_PATH]),
        receipt(config=["{}", CONFIG_PATH], warnings=warning_texts(3)),
        receipt(config=["{}", CONFIG_PATH], warnings=warning_texts(10)),
        receipt(warnings=["x" * 9000, *warning_texts(7)]),
        receipt(warnings=["a\nforged warning"]),
    ]


def lines_body(count: int) -> str:
    """A body of numbered lines."""
    return "".join(f"line {number}\n" for number in range(count))


def text_cases() -> list[object]:
    """Receipt, body and ceiling for each wrapped text."""
    cases: list[list[Any]] = [[row, "body\n", CEILING] for row in receipts()]
    cases.extend([receipt(), body, CEILING] for body in ("one\ntwo", "", "\n"))
    cases.extend([receipt(), lines_body(40), ceiling] for ceiling in (400, 120, 40, 8, 0))
    cases.append([receipt(), f"a\x0bb\x0cc{LINE_SEPARATOR}d\r\ne\n" * 30, 120])
    cases.append([receipt(config=["{}", CONFIG_PATH], warnings=warning_texts(10)), "x\n", 200])
    return [[row, body, ceiling, wrapped_text(row, body, ceiling)] for row, body, ceiling in cases]


def wrapped_text(row: dict[str, Any], body: str, ceiling: int) -> str:
    """The wrapped text of one case."""
    return envelope.wrap(context(row), body, counter=COUNTER, max_tokens=ceiling)


def detailed_report() -> list[dict[str, Any]]:
    """A report with each detail form and each observed field."""
    unit = {
        "unit": "f",
        "match": "name",
        "cc_old": 4,
        "cc_new": 11,
        "sloc_old": 11,
        "sloc_new": 19,
        "threshold": 10,
    }
    new_unit = {**unit, "unit": "g", "match": "new", "cc_old": None, "sloc_old": None}
    sites = [{"path": "pricing.py", "line": 58, "end_line": 63}]
    copy = {"key": KEY, "line": 84, "end_line": 89, "sites": sites, "sites_total": 1}
    details = [
        unit,
        new_unit,
        {"rule": "except-pass", "unit": "f", "capture": "except LookupError: pass"},
        {"reason": "no_boundary_globs"},
        {"reason": "no_rules_for_language", "language": "go"},
        {"reason": "no_tag_cache"},
        copy,
    ]
    observed = {
        "units_over_threshold": [{"path": "a.py", "unit": "f", "line": 3, "cc": 11, "sloc": 19}],
        "units_changed": [
            {
                "path": "a.py",
                "unit": "g",
                "line": 9,
                "match": "new",
                "cc_old": None,
                "cc_new": 2,
                "sloc_old": None,
                "sloc_new": 4,
            }
        ],
        "contract_keys": [{"path": "a.py", "rule": "r", "unit": "<module>", "capture": "c"}],
        "clone_instances": [{"path": "a.py", "key": KEY, "line": 1, "end_line": 9}],
    }
    findings = [finding(detail=detail) for detail in details]
    escaped = finding(message=f"{E_ACUTE}t{E_ACUTE} {LINE_SEPARATOR}")
    return [
        candidate("detailed", findings, observed),
        candidate(f"plain {E_ACUTE}{EMOJI}", [escaped]),
        candidate("partial", [], {"units_changed": []}),
    ]


def json_cases() -> list[object]:
    """Receipt, lint report and ceiling for each wrapped JSON document."""
    many = [candidate(f"cand-{number}", [finding(line=number)]) for number in range(30)]
    big = [candidate("big", [finding(message="m" * 4000)])]
    cases: list[list[Any]] = [[row, [], CEILING] for row in receipts()]
    cases.extend(
        [
            [receipt(), detailed_report(), CEILING],
            [receipt(), many, 600],
            [receipt(), many, 1],
            [receipt(), big, 300],
            [receipt(), [], 40],
        ]
    )
    return [[row, rows, ceiling, wrapped_json(row, rows, ceiling)] for row, rows, ceiling in cases]


def wrapped_json(row: dict[str, Any], rows: list[dict[str, Any]], ceiling: int) -> str:
    """The wrapped JSON of one lint case."""
    payload = lint_view(rows).as_dict()
    return envelope.wrap_json(
        context(row), payload, counter=COUNTER, max_tokens=ceiling, items_key="candidates"
    )


def envelope_fixture() -> Fixture:
    """The wrapped text and the wrapped JSON."""
    return {"text": text_cases(), "json": json_cases()}


LANGUAGES = [
    ("python", 14, True, True, ""),
    ("json", None, False, False, "not warmed: unavailable"),
    ("sql", 14, True, False, "probe failed"),
    ("go", 0, True, True, ""),
    ("rust", 15, True, True, "a note"),
    ("bash", 14, False, True, ""),
]


def language_rows(languages: list[tuple[str, int | None, bool, bool, str]]) -> list[object]:
    """One row per language, with the tier that Python derives from its name."""
    return [
        {
            "name": name,
            "tier": LanguageCapability(name, abi, "1.14.3", warmed, probe_ok, detail).tier,
            "abi": abi,
            "warmed": warmed,
            "probe_ok": probe_ok,
            "detail": detail,
        }
        for name, abi, warmed, probe_ok, detail in languages
    ]


def capability_row(config: list[Any], **overrides: Any) -> dict[str, Any]:
    """One capabilities report row, with the registry, the caps and the settings from Python."""
    settings = capability_service._effective_config(parsed_config(config))
    fields: dict[str, Any] = {
        "version": "0.11.0-rc.1",
        "pack_version": "1.14.3",
        "grammar_cache": "(linked into the binary)",
        "cache": {
            "receipt": "none",
            "path": "/cache/agentless-mcp/0123456789abcdef/tags.db",
            "generation": None,
            "repo_generation": None,
            "generation_matches": False,
            "enabled": True,
            "files": 0,
            "tags": 0,
            "note": "",
        },
        "cache_hint": "build it: agentless-mcp index --repo /srv/app",
        "languages": language_rows(LANGUAGES),
        "extensions": sorted(TreeSitterExtractor.SUPPORTED_EXTENSIONS.items()),
        "scope_analysed_languages": sorted(SCOPE_ANALYSED_LANGUAGES),
        "lint_checks": [[check.name, check.needs.value] for check in lint_service.CHECKS],
        "config": config,
        "effective_config": [
            [name, list(value) if isinstance(value, tuple) else value] for name, value in settings
        ],
        "caps": [list(pair) for pair in capability_service._caps()],
        "roots": {"configured": None, "client": []},
    }
    return {**fields, **overrides}


def capability_report(row: dict[str, Any]) -> capability_service.CapabilityReport:
    """The Python report of a row; the cache status is a stand-in with the row's values."""
    cache = row["cache"]
    status = SimpleNamespace(**cache, as_dict=lambda: dict(cache))
    roots = row["roots"]
    languages = tuple(
        LanguageCapability(
            language["name"],
            language["abi"],
            row["pack_version"],
            language["warmed"],
            language["probe_ok"],
            language["detail"],
        )
        for language in row["languages"]
    )
    return capability_service.CapabilityReport(
        version=row["version"],
        pack_version=row["pack_version"],
        grammar_cache=row["grammar_cache"],
        cache_status=status,
        cache_hint=row["cache_hint"],
        languages=languages,
        extensions=tuple(tuple(pair) for pair in row["extensions"]),
        config=parsed_config(row["config"]),
        effective_config=tuple(
            (name, tuple(value) if isinstance(value, list) else value)
            for name, value in row["effective_config"]
        ),
        caps=tuple(tuple(pair) for pair in row["caps"]),
        configured_roots=None if roots["configured"] is None else tuple(roots["configured"]),
        client_roots=tuple(roots["client"]),
        lint_checks=tuple(tuple(pair) for pair in row["lint_checks"]),
    )


def capability_rows() -> list[dict[str, Any]]:
    """Reports that cover each roots form, each language state and each setting form."""
    plain = ["{}", None]
    fresh = {
        "receipt": "g:1a2b3c4d fresh",
        "path": "/cache/agentless-mcp/0123456789abcdef/tags.db",
        "generation": "1a2b3c4d",
        "repo_generation": "1a2b3c4d",
        "generation_matches": True,
        "enabled": True,
        "files": 12,
        "tags": 345,
        "note": "",
    }
    ready = language_rows([("python", 14, True, True, ""), ("go", 15, True, True, "")])
    return [
        capability_row(plain),
        capability_row(plain, roots={"configured": [], "client": ["/x", "/y"]}),
        capability_row(plain, roots={"configured": ["/srv/a", "/srv/b"], "client": []}),
        capability_row(plain, roots={"configured": [""], "client": [""]}),
        capability_row([GOOD_CONFIG, CONFIG_PATH], cache=fresh, cache_hint=""),
        capability_row([BAD_CONFIG, CONFIG_PATH], languages=ready),
        capability_row(plain, languages=[]),
    ]


def capability_text(row: dict[str, Any]) -> str:
    """The capabilities body of one row."""
    return capability_service.render_capability_report(capability_report(row))


def capabilities_fixture() -> Fixture:
    """The capabilities body of each row, the JSON at four ceilings, and one wrapped text."""
    rows = capability_rows()
    first = rows[0]
    whole = capability_json(receipt(), first, CEILING)
    # 60 tokens under the whole document, so the ceiling drops some languages and keeps others.
    trimmed = COUNTER.count(whole) - 60
    cases: list[tuple[dict[str, Any], int]] = [
        (first, CEILING),
        (rows[4], CEILING),
        (first, trimmed),
        (first, 200),
    ]
    documents = [
        [receipt(), row, ceiling, capability_json(receipt(), row, ceiling)]
        for row, ceiling in cases
    ]
    wrapped = envelope.wrap(context(receipt()), capability_text(first), counter=COUNTER)
    return {
        "text": [[row, capability_text(row)] for row in rows],
        "json": documents,
        "wrapped": [[receipt(), first, wrapped]],
    }


def capability_json(receipt_row: dict[str, Any], row: dict[str, Any], ceiling: int) -> str:
    """The wrapped JSON of one capabilities case."""
    payload = capability_report(row).as_dict()
    return envelope.wrap_json(
        context(receipt_row), payload, counter=COUNTER, max_tokens=ceiling, items_key="languages"
    )


# Each vote: [id, index, applied, measured, key, regression passed, reproduction passed] rows and
# repro_valid. They cover every tier, the exclusions, the plurals and short and non-ASCII keys.
VOTES: list[tuple[list[list[Any]], bool]] = [
    ([], False),
    ([["a", 0, False, False, None, False, False]], True),
    ([["only", 0, True, True, "k" * 64, True, False]], False),
    (
        [
            ["01-plus", 0, True, True, "a" * 64, True, True],
            ["02-again", 1, True, True, "a" * 64, True, True],
            ["03-other", 2, True, True, "b" * 64, True, False],
            ["04-broken", 3, True, True, "c" * 64, False, False],
            ["05-misses", 4, False, False, None, False, False],
            ["06-unmeasured", 5, True, False, "d" * 64, False, False],
            ["07-empty", 6, True, True, None, True, True],
        ],
        True,
    ),
    (
        [
            ["late", 9, True, True, "short", True, False],
            ["early", 2, True, True, E_ACUTE * 20, True, False],
            ["tied", 5, True, True, "short", True, False],
        ],
        False,
    ),
    (
        [["x", 0, True, True, "e" * 64, False, False], ["y", 1, True, True, "f" * 64, False, True]],
        True,
    ),
]

# Each check: [path, verdict or None, error] rows; a verdict is [language, old errors, new errors,
# ok, detail, checked, first error line].
CHECKS: list[list[list[Any]]] = [
    [],
    [["gone.py", None, "no such file in this repository"]],
    [["a.py", ["python", 0, 0, True, "", True, None], ""]],
    [
        ["b.py", ["python", 1, 3, False, "new parse errors", True, 4], ""],
        ["n.rst", ["unknown", 0, 0, True, "no grammar: not checked", False, None], ""],
    ],
]


def vote_case(rows: list[list[Any]], repro_valid: bool) -> list[object]:
    """The vote text and summary of one candidate set."""
    report = vote.rank([vote.VoteCandidate(*row) for row in rows], repro_valid=repro_valid)
    return [rows, repro_valid, report.text(), report.summary_line()]


def check_case(files: list[list[Any]]) -> list[object]:
    """The check lines of one report, as the patch check command prints them."""
    checks = tuple(
        FileCheck(path, None if verdict is None else SyntaxVerdict(*verdict), error)
        for path, verdict, error in files
    )
    report = CheckReport(checks, ApplyResult((), {}))
    return [files, "\n".join(_check_lines(report)) + "\n"]


def write_fixture() -> Fixture:
    """The vote text and summary, and the check lines, of the write commands."""
    return {
        "vote": [vote_case(rows, repro_valid) for rows, repro_valid in VOTES],
        "check": [check_case(files) for files in CHECKS],
    }


BUILDERS: dict[str, Callable[[], Fixture]] = {
    "textsafe": textsafe_fixture,
    "lint": lint_fixture,
    "envelope": envelope_fixture,
    "capabilities": capabilities_fixture,
    "write": write_fixture,
}


if __name__ == "__main__":
    refuse_real_caches()
    sys.exit(write_or_check(__doc__.splitlines()[0], FIXTURES, BUILDERS))
