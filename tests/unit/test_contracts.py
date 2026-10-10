"""contract_drift's rule file, its compiled queries, the glob matcher, and the hits per text."""

import itertools
import json
import re

import pytest

from agentless_mcp.core import complexity, contracts, grammars
from agentless_mcp.core.contracts import in_boundary
from agentless_mcp.prompts import CONTRACT_RULES, ContractRule, PromptDataError
from agentless_mcp.prompts.contract_rules import parse_contract_rules

ECMASCRIPT = {"nullish-empty-default", "or-empty-default", "catch-return-default", "catch-empty"}

V1_RULES = {
    "python": {
        "get-default-empty",
        "or-empty-default",
        "getattr-default",
        "except-return-default",
        "except-pass",
    },
    "javascript": ECMASCRIPT,
    "typescript": ECMASCRIPT,
    "tsx": ECMASCRIPT,
    "go": {"err-check-returns-nil"},
    "rust": {"unwrap-or-default", "unwrap-or-empty", "ok-statement"},
    "java": {"catch-empty", "catch-return-null", "get-or-default-empty"},
    "ruby": {"rescue-nil", "fetch-default-empty", "or-empty-default"},
    "lua": {"or-empty-table", "pcall-statement"},
    "cpp": {"catch-all-empty"},
}


def src(*lines):
    return "\n".join(lines) + "\n"


ECMASCRIPT_CASES = [
    (
        "nullish-empty-default",
        src("const a = x ?? [];", "const b = y ?? {};"),
        ["x ?? []", "y ?? {}"],
        src(
            "const a = x ?? [1];",
            "const b = y ?? defaults;",
            "const c = x || [];",
            "const d = x ?? { };",
        ),
    ),
    (
        "or-empty-default",
        src("const a = x || [];", "const b = y || {};"),
        ["x || []", "y || {}"],
        src("const a = x || [1];", "const b = y && {};", "const c = x ?? {};"),
    ),
    (
        "catch-return-default",
        src(
            "function f() {",
            "  try { g(); } catch (e) { log(e); return null; }",
            "  try { g(); } catch { return []; }",
            "}",
        ),
        ["return null;", "return [];"],
        src(
            "function f() {",
            "  try { return null; } catch (e) { return fallback(e); }",
            "  try { g(); } catch (e) { if (x) { return null; } throw e; }",
            "}",
        ),
    ),
    (
        "catch-empty",
        src("try { g(); } catch (e) {}", "try { g(); } catch { /* ignore */ }"),
        ["catch (e) {}", "catch { }"],
        src("try { g(); } catch (e) { log(e); }", "try { g(); } catch (e) { // a", " // b", " }"),
    ),
]

# (language, rule, a source with hits, the captures it gives, a source with none)
CASES = [
    (
        "python",
        "get-default-empty",
        src(
            'a = data.get("items", [])',
            'b = data.get("k", {})',
            'c = d.get(k, "")',
            "e = d.get(k, 0)",
            "f = d.get(k, '')",
        ),
        [
            'data.get("items", [])',
            'data.get("k", {})',
            'd.get(k, "")',
            "d.get(k, 0)",
            "d.get(k, '')",
        ],
        src(
            'a = data.get("items")',
            'b = data.get("k", [1])',
            "c = d.get(k, None)",
            "e = d.get(k, default)",
            "f = get(k, [])",
            "g = d.fetch(k, [])",
            "h = d.get(k, [ ])",
        ),
    ),
    (
        "python",
        "or-empty-default",
        src("a = x or {}", "b = y or []", "c = p or q or {}"),
        ["x or {}", "y or []", "p or q or {}"],
        src("a = x or {1: 2}", "b = y and []", "c = z or default", "d = x or ''"),
    ),
    (
        "python",
        "getattr-default",
        src('a = getattr(o, "n", None)', "b = getattr(o, name, 3)"),
        ['getattr(o, "n", None)', "getattr(o, name, 3)"],
        src('a = getattr(o, "n")', 'b = obj.getattr(o, "n", None)', 'c = hasattr(o, "n")'),
    ),
    (
        "python",
        "except-return-default",
        src(
            "def f():",
            "    try:",
            "        g()",
            "    except KeyError:",
            "        return None",
            "    except ValueError:",
            "        log()",
            "        return []",
            "    except OSError:",
            "        return {}",
            "    except E:",
            "        return False",
            "    except F:",
            "        return 0",
        ),
        ["return None", "return []", "return {}", "return False", "return 0"],
        src(
            "def f():",
            "    try:",
            "        return None",
            "    except KeyError:",
            "        return fallback()",
            "    except E:",
            "        if x:",
            "            return None",
            "        raise",
            "    except F:",
            "        return",
            "    except G:",
            "        return None, 1",
        ),
    ),
    (
        "python",
        "except-pass",
        src(
            "try:",
            "    g()",
            "except KeyError:",
            "    pass",
            "except ValueError:  # c",
            "    pass  # ignore",
            "except:",
            "    # why",
            "    pass",
            "except OSError:",
            "    pass",
            "    # trailing",
        ),
        [
            "except KeyError: pass",
            "except ValueError: pass",
            "except: pass",
            "except OSError: pass",
        ],
        src(
            "try:",
            "    g()",
            "except KeyError:",
            "    log()",
            "except ValueError:",
            "    pass",
            "    g()",
            "for x in y:",
            "    pass",
        ),
    ),
    *[("javascript", *case) for case in ECMASCRIPT_CASES],
    *[("typescript", *case) for case in ECMASCRIPT_CASES],
    *[("tsx", *case) for case in ECMASCRIPT_CASES],
    (
        "go",
        "err-check-returns-nil",
        src(
            "package p",
            "",
            "func f() (int, error) {",
            "\tv, err := g()",
            "\tif err != nil {",
            "\t\treturn 0, nil",
            "\t}",
            "\tif err != nil {",
            "\t\tlog(err)",
            "\t\treturn nil",
            "\t}",
            "\treturn v, nil",
            "}",
        ),
        ["return 0, nil", "return nil"],
        src(
            "package p",
            "",
            "func f() (*T, error) {",
            "\tv, err := g()",
            "\tif err != nil {",
            "\t\treturn nil, err",
            "\t}",
            "\tif e != nil {",
            "\t\treturn nil",
            "\t}",
            "\tif err == nil {",
            "\t\treturn nil",
            "\t}",
            "\tif err != nil {",
            "\t\tif x {",
            "\t\t\treturn nil",
            "\t\t}",
            "\t\treturn v, err",
            "\t}",
            "\treturn v, nil",
            "}",
        ),
    ),
    (
        "rust",
        "unwrap-or-default",
        src(
            "fn f() {",
            "    let a = x.unwrap_or_default();",
            "    let b = y.get(1).unwrap_or_default();",
            "}",
        ),
        ["x.unwrap_or_default()", "y.get(1).unwrap_or_default()"],
        src(
            "fn f() {",
            "    let a = x.unwrap_or(1);",
            "    let b = unwrap_or_default(y);",
            "    let c = x.unwrap();",
            "}",
        ),
    ),
    (
        "rust",
        "unwrap-or-empty",
        src(
            "fn f() {",
            "    let a = x.unwrap_or(vec![]);",
            "    let b = y.unwrap_or(String::new());",
            '    let c = z.unwrap_or("");',
            "    let d = w.unwrap_or(0);",
            "    let e = v.unwrap_or(Default::default());",
            "    let g = u.unwrap_or(HashMap::new());",
            "}",
        ),
        [
            "x.unwrap_or(vec![])",
            "y.unwrap_or(String::new())",
            'z.unwrap_or("")',
            "w.unwrap_or(0)",
            "v.unwrap_or(Default::default())",
            "u.unwrap_or(HashMap::new())",
        ],
        src(
            "fn f() {",
            "    let a = x.unwrap_or(vec![1]);",
            "    let b = y.unwrap_or(Foo::new());",
            '    let c = z.unwrap_or("x");',
            "    let d = w.unwrap_or(fallback);",
            "    let e = v.unwrap_or_else(Vec::new);",
            "}",
        ),
    ),
    (
        "rust",
        "ok-statement",
        src("fn f() {", "    w.ok();", "    tx.send(1).ok();", "}"),
        ["w.ok()", "tx.send(1).ok()"],
        src(
            "fn f() {",
            "    let a = w.ok();",
            "    let b = w.ok()?;",
            "    if w.ok().is_some() {}",
            "    ok();",
            "}",
        ),
    ),
    (
        "java",
        "catch-empty",
        src(
            "class A {",
            "  void f() {",
            "    try { g(); } catch (IOException e) {}",
            "    try { g(); } catch (Exception e) { // ok",
            "    }",
            "    try { g(); } catch (E e) { /* x */ }",
            "  }",
            "}",
        ),
        ["catch (IOException e) {}", "catch (Exception e) { }", "catch (E e) { }"],
        src(
            "class A {",
            "  void f() {",
            "    try { g(); } catch (IOException e) { log(e); }",
            "    try { } finally { }",
            "  }",
            "}",
        ),
    ),
    (
        "java",
        "catch-return-null",
        src(
            "class A {",
            "  Object f() {",
            "    try { return g(); } catch (E e) { log(e); return null; }",
            "  }",
            "}",
        ),
        ["return null;"],
        src(
            "class A {",
            "  Object f() {",
            "    try { return null; } catch (E e) { return fallback(); }",
            "  }",
            "}",
        ),
    ),
    (
        "java",
        "get-or-default-empty",
        src(
            "class A {",
            "  void f() {",
            "    a = m.getOrDefault(k, List.of());",
            '    b = m.getOrDefault(k, "");',
            "    c = m.getOrDefault(k, 0);",
            "    d = m.getOrDefault(k, Collections.emptyList());",
            "    e = m.getOrDefault(k, new ArrayList<>());",
            "  }",
            "}",
        ),
        [
            "m.getOrDefault(k, List.of())",
            'm.getOrDefault(k, "")',
            "m.getOrDefault(k, 0)",
            "m.getOrDefault(k, Collections.emptyList())",
            "m.getOrDefault(k, new ArrayList<>())",
        ],
        src(
            "class A {",
            "  void f() {",
            "    a = m.getOrDefault(k, List.of(1));",
            '    b = m.getOrDefault(k, "x");',
            "    c = m.getOrDefault(k, fallback);",
            "    d = m.get(k);",
            "    e = m.getOrDefault(k, new Foo());",
            "  }",
            "}",
        ),
    ),
    (
        "ruby",
        "rescue-nil",
        src(
            "def f",
            "  a = g rescue nil",
            "  begin",
            "    z",
            "  rescue StandardError => e",
            "    nil",
            "  end",
            "end",
            "",
            "def k",
            "  q",
            "rescue",
            "  nil",
            "end",
        ),
        ["g rescue nil", "rescue StandardError => e nil", "rescue nil"],
        src(
            "def f",
            "  a = g rescue default",
            "  begin",
            "    z",
            "  rescue StandardError => e",
            "    log(e)",
            "    nil",
            "  end",
            "end",
            "x = nil",
        ),
    ),
    (
        "ruby",
        "fetch-default-empty",
        src(
            "a = h.fetch(:k, [])",
            'b = h.fetch("k", {})',
            'c = h.fetch(:k, "")',
            "d = h.fetch(:k, 0)",
        ),
        ["h.fetch(:k, [])", 'h.fetch("k", {})', 'h.fetch(:k, "")', "h.fetch(:k, 0)"],
        src(
            "a = h.fetch(:k)", "b = h.fetch(:k, [1])", "c = h.fetch(:k) { [] }", "d = h.get(:k, [])"
        ),
    ),
    (
        "ruby",
        "or-empty-default",
        src("a = x || {}", "b = y || []"),
        ["x || {}", "y || []"],
        src("a = x || [1]", "b = y && []", "c = z or []", "d ||= []"),
    ),
    (
        "lua",
        "or-empty-table",
        src("local t = x or {}", "local u = a.b or {}"),
        ["x or {}", "a.b or {}"],
        src("local t = x or {1}", "local u = x and {}", "local v = x or default"),
    ),
    (
        "lua",
        "pcall-statement",
        src("pcall(f, 1)", "function h()", "  pcall(f)", "end", "if x then pcall(g) end"),
        ["pcall(f, 1)", "pcall(f)", "pcall(g)"],
        src(
            "local ok = pcall(f)",
            "local ok, err = pcall(g)",
            "if pcall(f) then end",
            "xpcall(f, h)",
            "return pcall(f)",
        ),
    ),
    (
        "cpp",
        "catch-all-empty",
        src(
            "void f() {",
            "  try { g(); } catch (...) {}",
            "  try { g(); } catch (...) { // ignore",
            "  }",
            "}",
        ),
        ["catch (...) {}", "catch (...) { }"],
        src(
            "void f() {",
            "  try { g(); } catch (const E& e) {}",
            "  try { g(); } catch (...) { log(); }",
            "  try { g(); } catch (...) { throw; }",
            "}",
        ),
    ),
]

VALID_RULE = {
    "id": "r",
    "languages": ["python"],
    "severity": "warning",
    "query": "(identifier) @hit",
    "message": "A message.",
}


def captures(language, rule_id, source):
    units = complexity.measure(source, "sample", language)
    return [
        hit.capture for hit in contracts.hits(source, language, units) if hit.rule.id == rule_id
    ]


def parse(document):
    return parse_contract_rules("rules.json", json.dumps(document))


def rule(query, language="python"):
    return ContractRule(
        id="probe", language=language, severity="warning", query=query, message="A message."
    )


class TestTheRuleFile:
    def test_the_packaged_file_loads_one_record_per_rule_and_language(self):
        assert len(CONTRACT_RULES) == sum(len(ids) for ids in V1_RULES.values())

    def test_a_list_of_languages_expands_to_one_record_each(self):
        entry = {**VALID_RULE, "languages": ["javascript", "tsx"]}
        assert [record.language for record in parse({"rules": [entry]})] == ["javascript", "tsx"]

    @pytest.mark.parametrize(
        ("document", "reason"),
        [
            ({"rules": [VALID_RULE], "extra": 1}, r"unknown \['extra'\]"),
            ({"rules": [{**VALID_RULE, "lang": "python"}]}, r"unknown \['lang'\]"),
            ({"rules": [{k: v for k, v in VALID_RULE.items() if k != "query"}]}, "missing"),
            ({"rules": []}, "must be a non-empty list"),
            ({"rules": ["not an object"]}, "must be a JSON object"),
            ({"rules": [{**VALID_RULE, "languages": "python"}]}, "'languages' must be"),
            ({"rules": [{**VALID_RULE, "languages": []}]}, "'languages' must be"),
            ({"rules": [{**VALID_RULE, "languages": ["go", "go"]}]}, "'languages' must be"),
            ({"rules": [{**VALID_RULE, "severity": "not_checked"}]}, "severity must be one of"),
            ({"rules": [{**VALID_RULE, "query": "  "}]}, "'query' must be a non-empty string"),
            ({"rules": [VALID_RULE, VALID_RULE]}, "defines rule 'r' twice for python"),
        ],
        ids=[
            "unknown-top-level-key",
            "unknown-rule-key",
            "missing-rule-key",
            "no-rules",
            "rule-not-an-object",
            "languages-not-a-list",
            "languages-empty",
            "language-twice",
            "severity-not-allowed",
            "blank-query",
            "rule-twice",
        ],
    )
    def test_a_defect_fails_to_load(self, document, reason):
        with pytest.raises(PromptDataError, match=reason):
            parse(document)

    def test_text_that_is_not_json_fails_to_load(self):
        with pytest.raises(PromptDataError, match="is not valid JSON"):
            parse_contract_rules("rules.json", "{not json")

    def test_a_rule_for_a_language_without_pair_rules_fails_the_index(self):
        with pytest.raises(PromptDataError, match="names language 'cobol'"):
            contracts._index([rule("(identifier) @hit", language="cobol")])


class TestEveryQueryCompiles:
    """The gate for rule wiring: queries compile lazily, so only this test sees them all."""

    @pytest.mark.parametrize(
        "record", CONTRACT_RULES, ids=[f"{r.language}:{r.id}" for r in CONTRACT_RULES]
    )
    def test_every_rule_compiles_against_its_grammar(self, record):
        compiled = contracts._compile(record, grammars.get_language(record.language))
        assert compiled.rule == record

    def test_the_rule_ids_per_language_are_the_v1_set(self):
        found: dict[str, set[str]] = {}
        for record in CONTRACT_RULES:
            found.setdefault(record.language, set()).add(record.id)
        assert found == V1_RULES

    def test_every_tier1_language_but_c_and_bash_has_rules(self):
        assert set(V1_RULES) == set(grammars.TIER1_LANGUAGES) - {"c", "bash"}
        assert contracts.rules_for("c") == ()
        assert contracts.rules_for("bash") == ()
        assert contracts.compiled("bash") == ()

    @pytest.mark.parametrize(
        ("query", "reason"),
        [
            ("(no_such_node) @hit", "does not compile"),
            ("(identifier) @found", "must capture @hit"),
            ("(identifier) @hit (identifier) @name", "must capture @hit"),
            ('((identifier) @hit (#bogus? @hit "x"))', r"uses predicates \['bogus\?'\]"),
        ],
        ids=["unknown-node", "no-hit-capture", "unprefixed-capture", "unknown-predicate"],
    )
    def test_a_rule_the_compiler_or_the_checks_refuse_names_itself(self, query, reason):
        with pytest.raises(PromptDataError, match=rf"contract rule 'probe' for python {reason}"):
            contracts._compile(rule(query), grammars.get_language("python"))


class TestEveryRule:
    @pytest.mark.parametrize(
        ("language", "rule_id", "positive", "expected", "negative"),
        CASES,
        ids=[f"{case[0]}:{case[1]}" for case in CASES],
    )
    def test_a_rule_matches_its_syntax_and_nothing_else(
        self, language, rule_id, positive, expected, negative
    ):
        assert captures(language, rule_id, positive) == expected
        assert captures(language, rule_id, negative) == []

    def test_every_rule_has_a_case(self):
        assert {(case[0], case[1]) for case in CASES} == {
            (record.language, record.id) for record in CONTRACT_RULES
        }


class TestHits:
    def test_a_hit_takes_the_innermost_unit_that_holds_it(self):
        source = (
            "class Client:\n"
            "    def fetch(self, data):\n"
            "        def inner():\n"
            '            return data.get("a", [])\n'
            '        return data.get("b", {})\n'
            'TOP = CONFIG.get("c", "")\n'
        )
        units = complexity.measure(source, "client.py", "python")
        found = [
            (hit.line, None if hit.unit is None else hit.unit.qualname)
            for hit in contracts.hits(source, "python", units)
        ]
        assert found == [(4, "Client.fetch.inner"), (5, "Client.fetch"), (6, None)]

    def test_a_capture_collapses_whitespace_and_drops_comments(self):
        source = src("x = (", "    first  # the left side", "    or   []", ")")
        assert captures("python", "or-empty-default", source) == ["first or []"]

    def test_a_language_without_rules_has_no_hits(self):
        assert contracts.hits("echo hi\n", "bash", ()) == ()


class TestBoundaryGlobs:
    @pytest.mark.parametrize(
        ("glob", "path", "expected"),
        [
            ("*.py", "a.py", True),
            ("*.py", "src/a.py", False),
            ("**/adapters/**", "adapters/x.py", True),
            ("**/adapters/**", "src/pkg/adapters/x.py", True),
            ("**/adapters/**", "src/pkg/adapters/deep/x.py", True),
            ("**/adapters/**", "src/myadapters/x.py", False),
            ("**/adapters/**", "adaptersx/y.py", False),
            ("**/adapters/**", "adapters", False),
            ("src/**/x.py", "src/x.py", True),
            ("src/**/x.py", "src/a/b/x.py", True),
            ("src/*", "src/a.py", True),
            ("src/*", "src/a/b.py", False),
            ("src/*/api.py", "src/pkg/api.py", True),
            ("src/*/api.py", "src/pkg/sub/api.py", False),
            ("**", "any/path/at/all.py", True),
            ("a.py", "aXpy", False),
            ("pages/[id].tsx", "pages/[id].tsx", True),
            ("pages/[id].tsx", "pages/i.tsx", False),
            ("api?.py", "api1.py", False),
            ("Src/*.py", "src/a.py", False),
        ],
    )
    def test_a_glob_matches_by_its_segments(self, glob, path, expected):
        assert in_boundary(path, [glob]) is expected

    def test_any_glob_in_the_list_can_match(self):
        assert in_boundary("lib/clients/http.py", ["src/**", "**/clients/*.py"])

    def test_no_globs_match_nothing(self):
        assert not in_boundary("a.py", [])

    # The suite's per-test timeout fails a matcher that backtracks on these two.
    def test_twenty_stars_in_one_segment_on_a_256_character_path(self):
        glob = "a*" * 20 + "b"

        assert not in_boundary("a" * 256, [glob])
        assert in_boundary("a" * 255 + "b", [glob])

    def test_twenty_stars_across_segments_on_a_256_character_path(self):
        glob = "/".join(["**"] * 10) + "/b"
        path = "/".join(["aa"] + ["a"] * 127)

        assert len(path) == 256
        assert not in_boundary(path, [glob])
        assert in_boundary(path[:-1] + "b", [glob])

    def test_the_matcher_agrees_with_the_regex_it_replaced(self):
        globs = ["/".join(parts) for size in (1, 2, 3) for parts in product(GLOB_ELEMENTS, size)]
        paths = ["/".join(parts) for size in (1, 2, 3, 4) for parts in product(SEGMENTS, size)]

        disagree = [
            (glob, path)
            for glob in globs
            for pattern in [regex_oracle(glob)]
            for path in paths
            if in_boundary(path, [glob]) != bool(pattern.fullmatch(path))
        ]

        assert disagree == []


GLOB_ELEMENTS = ("**", "*", "a", "a*", "*a*", "a**", "")
SEGMENTS = ("a", "b", "ab", "")


def product(items, size):
    return itertools.product(items, repeat=size)


def regex_oracle(glob):
    """The regex that in_boundary compiled before the segment matcher: its exact semantics."""
    segments = glob.split("/")
    last = len(segments) - 1
    parts = []
    for index, segment in enumerate(segments):
        if segment == "**":
            parts.append("[^/]+(?:/[^/]+)*" if index == last else "(?:[^/]+/)*")
        else:
            literal = "[^/]*".join(re.escape(piece) for piece in segment.split("*"))
            parts.append(literal if index == last else f"{literal}/")
    return re.compile("".join(parts))
