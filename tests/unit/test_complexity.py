import dataclasses
import sys
from pathlib import Path

import pytest

from agentless_mcp.core import grammars
from agentless_mcp.core.complexity import RULES, Unit, erosion, measure, measure_tree
from agentless_mcp.core.extractor import TreeSitterExtractor
from agentless_mcp.util.errors import UnmeasurableSource

FIXTURES = Path(__file__).parent.parent / "characterization" / "fixtures"

# Hand-counted by McConnell's rule and the D1 tables: (qualname, start_line, cc, sloc).
HAND_COUNTED = {
    "bash": ("sample.sh", [("check", 3, 10, 18), ("count", 24, 9, 13)]),
    "c": ("sample.c", [("find", 3, 4, 10), ("classify", 16, 7, 18)]),
    "cpp": ("sample.cpp", [("geo.Shape.area", 4, 6, 11), ("geo.Shape.scale", 21, 3, 7)]),
    "go": ("sample.go", [("Store.Find", 5, 7, 18), ("Classify", 27, 5, 15)]),
    "java": (
        "Sample.java",
        [("Sample.route", 2, 8, 18), ("Sample.loop", 24, 7, 17), ("Sample.Point.Point", 43, 2, 3)],
    ),
    "javascript": (
        "sample.js",
        [
            ("route", 2, 8, 16),
            ("pick", 22, 2, 1),
            ("outer", 24, 3, 6),
            ("outer.inner", 25, 2, 3),
            ("Box.size", 32, 3, 7),
            ("Box.open", 40, 4, 9),
            ("load", 52, 2, 3),
        ],
    ),
    "lua": ("sample.lua", [("clamp", 3, 3, 10), ("M.pick", 17, 8, 14), ("M.handler", 32, 2, 3)]),
    "ruby": (
        "sample.rb",
        [
            ("Router.route", 2, 5, 11),
            ("Router.classify", 17, 6, 12),
            ("Router.pick", 30, 5, 7),
            ("Router.drain", 38, 7, 10),
        ],
    ),
    "rust": (
        "sample.rs",
        [
            ("Parser.next", 6, 7, 12),
            ("Parser.drain", 22, 5, 12),
            ("outer", 36, 2, 6),
            ("outer.inner", 37, 2, 3),
        ],
    ),
    "tsx": ("sample.tsx", [("Row", 1, 3, 5), ("List", 10, 2, 7)]),
    "typescript": (
        "sample.ts",
        [("Store.load", 2, 2, 1), ("Store.find", 4, 4, 10), ("ids", 19, 2, 5)],
    ),
}


def rows(units):
    return [(unit.qualname, unit.start_line, unit.cc, unit.sloc) for unit in units]


def unit(cc, sloc):
    return Unit(
        qualname="f",
        path="a.py",
        start_byte=0,
        end_byte=1,
        start_line=1,
        end_line=1,
        body_key="",
        cc=cc,
        sloc=sloc,
    )


class TestRules:
    def test_the_rules_cover_exactly_the_tier1_languages(self):
        assert sorted(RULES) == sorted(grammars.TIER1_LANGUAGES)

    @pytest.mark.parametrize("language", sorted(RULES))
    def test_every_rule_name_is_a_named_kind(self, language):
        rules = RULES[language]
        names = (
            rules.units.units
            | rules.units.scopes
            | rules.units.bound
            | rules.units.reads
            | (rules.units.descend or frozenset())
            | rules.decisions.points
            | set(rules.decisions.conditional)
            | set(rules.decisions.field_operators)
            | set(rules.decisions.token_operators)
        )
        grammar = grammars.get_language(language)
        missing = sorted(name for name in names if grammar.id_for_node_kind(name, True) is None)
        assert not missing, f"{language} rules name kinds its grammar lacks: {missing}"

    @pytest.mark.parametrize("language", sorted(RULES))
    def test_every_token_operator_is_an_anonymous_kind(self, language):
        grammar = grammars.get_language(language)
        spellings = {
            token
            for tokens in RULES[language].decisions.token_operators.values()
            for token in tokens
        }
        missing = sorted(
            token for token in spellings if grammar.id_for_node_kind(token, False) is None
        )
        assert not missing


class TestHandCountedFixtures:
    @pytest.mark.parametrize("language", sorted(HAND_COUNTED))
    def test_every_unit_has_its_hand_counted_metrics(self, language):
        name, expected = HAND_COUNTED[language]
        path = FIXTURES / f"complexity_{language}" / name
        assert rows(measure(path.read_text(encoding="utf-8"), name, language)) == expected

    def test_the_hand_counts_cover_every_language_but_python(self):
        assert sorted(HAND_COUNTED) == sorted(set(RULES) - {"python"})


class TestUnits:
    def test_units_come_back_in_source_order_with_their_byte_spans(self):
        source = (
            "def outer():\n    def inner():\n        return 1\n    return inner\n"
            "\n\ndef last():\n    pass\n"
        )
        units = measure(source, "a.py", "python")
        assert [unit.qualname for unit in units] == ["outer", "outer.inner", "last"]
        data = source.encode("utf-8")
        assert all(data[unit.start_byte : unit.end_byte].startswith(b"def ") for unit in units)

    def test_a_bound_arrow_spans_the_callable_not_the_binding(self):
        source = "const pick = (a) => (a ? 1 : 2);\n"
        (picked,) = measure(source, "a.js", "javascript")
        assert source.encode("utf-8")[picked.start_byte : picked.end_byte] == b"(a) => (a ? 1 : 2)"
        assert (picked.qualname, picked.cc) == ("pick", 2)

    def test_same_name_units_carry_no_ordinal(self):
        source = "def f():\n    return 1\n\n\ndef f():\n    return 2\n"
        assert [unit.qualname for unit in measure(source, "a.py", "python")] == ["f", "f"]

    def test_an_anonymous_callable_counts_toward_its_enclosing_unit(self):
        source = (
            "def f(xs):\n    return sorted(xs, key=lambda x: x if x else 0)\n"
            "\n\ng = lambda y: y or 1\n"
        )
        assert rows(measure(source, "a.py", "python")) == [("f", 1, 2, 2)]

    def test_a_unit_is_a_frozen_plain_value(self):
        assert {field.type for field in dataclasses.fields(Unit)} == {str, int}
        frozen = unit(1, 1)
        with pytest.raises(dataclasses.FrozenInstanceError):
            frozen.cc = 2

    def test_mass_is_cc_times_the_square_root_of_sloc(self):
        assert unit(3, 16).mass == 12.0


class TestBodyKey:
    @staticmethod
    def key(source, language="python"):
        (only,) = measure(source, "a", language)
        return only.body_key

    def test_a_rename_keeps_the_key(self):
        assert self.key("def a(x):\n    return x + 1\n") == self.key(
            "def b(x):\n    return x + 1\n"
        )

    def test_a_comment_keeps_the_key(self):
        plain = "def a(x):\n    return x + 1\n"
        commented = "def a(x):\n    # One more.\n    return x + 1\n"
        assert self.key(plain) == self.key(commented)

    def test_a_body_change_moves_the_key(self):
        assert self.key("def a(x):\n    return x + 1\n") != self.key(
            "def a(x):\n    return x - 1\n"
        )

    def test_a_c_rename_keeps_the_key(self):
        before = self.key("int a(int x) { return x; }\n", "c")
        assert before == self.key("int b(int x) { return x; }\n", "c")


class TestErosion:
    def test_no_units_is_zero(self):
        assert erosion(()) == 0.0

    def test_no_mass_is_zero(self):
        assert erosion([unit(20, 0), unit(1, 0)]) == 0.0

    def test_the_threshold_is_strict(self):
        at, over = unit(10, 4), unit(11, 4)
        assert erosion([at, over]) == pytest.approx(over.mass / (at.mass + over.mass))

    def test_the_threshold_is_a_parameter(self):
        assert erosion([unit(10, 4), unit(11, 4)], threshold=9) == 1.0

    def test_it_accepts_a_one_shot_iterable(self):
        assert erosion(iter([unit(11, 4), unit(1, 4)])) == pytest.approx(22 / 24)


class TestFailures:
    @pytest.mark.parametrize(
        ("language", "source"),
        [
            ("python", "def f(:\n    pass\n"),
            ("javascript", "function f( {\n"),
            ("bash", "if [ x ]; then\n  echo\n"),
        ],
    )
    def test_source_that_does_not_parse_is_a_typed_failure(self, language, source):
        with pytest.raises(UnmeasurableSource, match="does not parse"):
            measure(source, "a", language)

    def test_a_dedent_the_tokenizer_rejects_is_a_typed_failure(self):
        source = "def f():\n    if x:\n        pass\n  pass\n"
        with pytest.raises(UnmeasurableSource, match="IndentationError"):
            measure(source, "a.py", "python")

    @pytest.mark.parametrize(
        "source",
        [
            pytest.param("if x:\n\tpass\n        pass\n", id="tabs-then-spaces"),
            pytest.param('x = "a\u2028b"\n', id="line-separator-in-a-one-line-string"),
        ],
    )
    def test_the_c_tokenizer_rejects_what_the_old_one_accepted(self, source):
        # Python 3.12 moved tokenize onto the C tokenizer (PEP 701), which
        # rejects these. Parity is defined from 3.12 on.
        if sys.version_info >= (3, 12):
            with pytest.raises(UnmeasurableSource, match="tokenizer rejects"):
                measure(source, "a.py", "python")
        else:
            assert measure(source, "a.py", "python") == ()

    def test_a_language_without_rules_is_a_typed_failure(self):
        with pytest.raises(UnmeasurableSource, match="no rules for language 'yaml'"):
            measure("x: 1\n", "a.yaml", "yaml")


class TestPythonSloc:
    def test_a_multiline_fstring_counts_by_the_running_tokenizer(self):
        # 3.12 tokenizes an f-string into parts (PEP 701); 3.10 and 3.11 see one token.
        source = 'def f(x):\n    return f"""a\n{x}\nb"""\n'
        (only,) = measure(source, "a.py", "python")
        assert only.sloc == (4 if sys.version_info >= (3, 12) else 2)

    @pytest.mark.parametrize(
        ("language", "source"),
        [
            ("python", 'def f(x):\n    """Doc."""\n    if x:\n        return 1\n    return 2\n'),
            ("c", "int f(int x) {\n  if (x) {\n    return 1;\n  }\n  return 2;\n}\n"),
        ],
    )
    def test_crlf_text_measures_like_lf(self, language, source):
        def shape(text):
            return [
                (unit.qualname, unit.start_line, unit.end_line, unit.cc, unit.sloc, unit.body_key)
                for unit in measure(text, "a", language)
            ]

        assert shape(source.replace("\n", "\r\n")) == shape(source)


def fixture_texts():
    for path in sorted(FIXTURES.glob("complexity_*/*")):
        language = TreeSitterExtractor.SUPPORTED_EXTENSIONS.get(path.suffix)
        if path.is_file() and language in RULES:
            yield pytest.param(path, language, id=f"{path.parent.name}/{path.name}")


def outcome(function, *arguments):
    try:
        return function(*arguments)
    except UnmeasurableSource as exc:
        return ("unmeasurable", str(exc))


def parsed(source, language):
    return grammars.get_parser(language).parse(source.encode("utf-8")).root_node


class TestTheTreeForm:
    @pytest.mark.parametrize(("path", "language"), list(fixture_texts()))
    def test_the_tree_form_equals_the_text_form_on_every_fixture(self, path, language):
        text = path.read_text(encoding="utf-8")

        from_tree = outcome(measure_tree, parsed(text, language), text, path.name, language)

        assert from_tree == outcome(measure, text, path.name, language)

    @pytest.mark.parametrize(
        ("language", "source"),
        [
            ("python", "def f(:\n    pass\n"),
            ("python", "def f():\n    if x:\n        pass\n  pass\n"),
            ("bash", "if [ x ]; then\n  echo\n"),
        ],
    )
    def test_the_tree_form_fails_with_the_text_forms_message(self, language, source):
        from_tree = outcome(measure_tree, parsed(source, language), source, "a", language)

        assert from_tree[0] == "unmeasurable"
        assert from_tree == outcome(measure, source, "a", language)

    def test_the_tree_form_names_a_language_without_rules(self):
        source = "x = 1\n"

        with pytest.raises(UnmeasurableSource, match="no rules for language 'yaml'"):
            measure_tree(parsed(source, "python"), source, "a.yaml", "yaml")

    def test_the_tree_form_never_parses(self, monkeypatch):
        source = "def f(x):\n    if x:\n        return 1\n    return 2\n"
        root = parsed(source, "python")

        def refuse(_language):
            message = "measure_tree parsed the source again"
            raise AssertionError(message)

        monkeypatch.setattr(grammars, "get_parser", refuse)

        assert rows(measure_tree(root, source, "a.py", "python")) == [("f", 1, 2, 4)]
