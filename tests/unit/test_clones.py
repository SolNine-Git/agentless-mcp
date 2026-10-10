from functools import partial
from pathlib import Path

import pytest

from agentless_mcp.core import clones, complexity, erosion, grammars
from agentless_mcp.core.clones import (
    CANDIDATES,
    clone_groups,
    clone_sloc_lines,
    file_blocks,
    file_blocks_tree,
)
from agentless_mcp.core.extractor import TreeSitterExtractor
from agentless_mcp.core.normalize import CLONE_IDENTIFIERS, CLONE_LITERALS, clone_key
from agentless_mcp.util.errors import UnmeasurableSource

FIXTURES = Path(__file__).parent.parent / "characterization" / "fixtures"
# Every Python version's tokenizer rejects this dedent, and tree-sitter parses it.
DEDENT_REJECTED = "def f():\n    if x:\n        pass\n  pass\n"

# Hand-read from each fixture: the first two functions are a Type-2 pair (names
# and literals changed), and the third is a near miss (one operator or one call).
HAND_GROUPED = {
    "bash": ("clones.sh", [[(3, 7), (9, 13)]]),
    "c": ("clones.c", [[(3, 7), (9, 13)]]),
    "cpp": ("clones.cpp", [[(3, 7), (9, 13)]]),
    "go": ("clones.go", [[(5, 9), (11, 15)]]),
    "java": ("Clones.java", [[(2, 6), (8, 12)]]),
    "javascript": ("clones.js", [[(1, 5), (7, 11)]]),
    "lua": ("clones.lua", [[(1, 5), (7, 11)]]),
    "ruby": ("clones.rb", [[(1, 5), (7, 11)]]),
    "rust": ("clones.rs", [[(1, 5), (7, 11)]]),
    "tsx": ("clones.tsx", [[(1, 4), (6, 9)]]),
    "typescript": ("clones.ts", [[(1, 5), (7, 11)]]),
}
NEAR_MISS_LINE = {
    "bash": 15,
    "c": 15,
    "cpp": 15,
    "go": 17,
    "java": 14,
    "javascript": 13,
    "lua": 13,
    "ruby": 13,
    "rust": 13,
    "tsx": 11,
    "typescript": 13,
}


def keys(source, language="python"):
    """The Type-2 key of each block of ``source``, in source order."""
    return [block.key for block in file_blocks(source, "a", language).blocks]


def only_key(source, language="python"):
    (key,) = keys(source, language)
    return key


def spans(source, language="python"):
    blocks = file_blocks(source, "a", language).blocks
    return [(block.start_line, block.end_line) for block in blocks]


TOTAL = (
    "def total(items):\n    out = 0\n    for item in items:\n        out += item\n    return out\n"
)


class TestTables:
    def test_the_tables_cover_exactly_the_tier1_languages(self):
        tier1 = sorted(grammars.TIER1_LANGUAGES)
        assert sorted(CANDIDATES) == tier1
        assert sorted(CLONE_IDENTIFIERS) == tier1
        assert sorted(CLONE_LITERALS) == tier1

    @pytest.mark.parametrize("language", sorted(CANDIDATES))
    def test_every_table_name_is_a_named_kind(self, language):
        rules = CANDIDATES[language]
        names = (
            rules.functions
            | rules.blocks
            | CLONE_IDENTIFIERS[language]
            | set(CLONE_LITERALS[language])
        )
        grammar = grammars.get_language(language)
        missing = sorted(name for name in names if grammar.id_for_node_kind(name, True) is None)
        assert not missing, f"{language} clone tables name kinds its grammar lacks: {missing}"

    def test_function_and_block_types_do_not_overlap(self):
        for language, rules in CANDIDATES.items():
            assert not rules.functions & rules.blocks, language


class TestTheTypeTwoKey:
    def test_renamed_identifiers_keep_the_key(self):
        renamed = TOTAL.replace("total", "add").replace("item", "value").replace("out", "acc")
        assert keys(TOTAL) == keys(renamed)

    def test_changed_literals_keep_the_key(self):
        assert keys(TOTAL) == keys(TOTAL.replace("out = 0", "out = 7"))
        source = 'def f(x):\n    y = "a"\n    return y\n'
        assert only_key(source) == only_key(source.replace('"a"', "'zzz'"))

    def test_a_literal_of_another_type_moves_the_key(self):
        source = "def f(x):\n    y = 1\n    return y\n"
        assert only_key(source) != only_key(source.replace("1", "1.5"))

    def test_a_changed_operator_moves_the_key(self):
        assert keys(TOTAL) != keys(TOTAL.replace("+=", "-="))

    def test_a_comment_keeps_the_key(self):
        commented = TOTAL.replace("    return out\n", "    # Done.\n    return out\n")
        assert keys(TOTAL) == keys(commented)

    def test_a_directive_comment_keeps_the_key(self):
        directive = TOTAL.replace("    return out\n", "    return out  # noqa: RET504\n")
        assert keys(TOTAL) == keys(directive)

    def test_names_are_numbered_by_first_appearance(self):
        first = "def f(a, b):\n    c = a + b\n    return a\n"
        assert only_key(first) != only_key(first.replace("return a", "return b"))

    def test_the_exact_key_moves_on_a_rename(self):
        (old,) = file_blocks("def f(a):\n    b = a\n    return b\n", "a", "python").blocks
        (new,) = file_blocks("def g(a):\n    b = a\n    return b\n", "a", "python").blocks
        assert old.key == new.key
        assert old.exact != new.exact


class TestPythonStatementDrops:
    WITH_DOC = 'def f(x):\n    """Doc."""\n    y = x\n    return y\n'

    def test_a_docstring_is_dropped(self):
        assert only_key(self.WITH_DOC) == only_key(
            'def f(x):\n    """Another doc."""\n    y = x\n    return y\n'
        )

    def test_a_docstring_and_no_docstring_share_a_key(self):
        bare = "def f(x):\n    y = x\n    w = 2\n    return y\n"
        documented = 'def f(x):\n    """Doc."""\n    y = x\n    w = 2\n    return y\n'
        assert only_key(bare) == only_key(documented)

    def test_a_docstring_that_does_not_own_its_line_is_still_dropped(self):
        # scb-check's clone rule has no ownership test, unlike its SLOC rule.
        bare = "def f(x):\n    y = x\n    w = 2\n    return y\n"
        trailing = 'def f(x):\n    """Doc."""  # note\n    y = x\n    w = 2\n    return y\n'
        assert only_key(bare) == only_key(trailing)

    @pytest.mark.parametrize("prefix", ["b", "f", "rb", "F"])
    def test_a_bytes_or_f_string_statement_is_kept(self, prefix):
        bare = "def f(x):\n    y = x\n    w = 2\n    return y\n"
        kept = f'def f(x):\n    {prefix}"doc"\n    y = x\n    w = 2\n    return y\n'
        assert only_key(bare) != only_key(kept)

    def test_a_one_element_string_tuple_is_dropped(self):
        bare = "def f(x):\n    y = x\n    w = 2\n    return y\n"
        dropped = 'def f(x):\n    "a",\n    y = x\n    w = 2\n    return y\n'
        assert only_key(bare) == only_key(dropped)

    def test_a_parenthesized_string_tuple_is_kept(self):
        bare = "def f(x):\n    y = x\n    w = 2\n    return y\n"
        kept = 'def f(x):\n    ("a",)\n    y = x\n    w = 2\n    return y\n'
        assert only_key(bare) != only_key(kept)

    def test_a_string_inside_an_expression_is_not_a_docstring(self):
        source = 'def f(x):\n    y = "a"\n    w = 2\n    return y\n'
        assert only_key(source) != only_key(source.replace('y = "a"', "y = None"))


class TestCandidates:
    def test_a_block_under_three_source_lines_is_not_a_candidate(self):
        assert spans("def f(x):\n    return x\n") == []

    def test_comments_and_blank_lines_do_not_count_toward_the_three(self):
        assert spans("def f(x):\n\n    # note\n\n    return x\n") == []

    def test_nested_blocks_are_candidates_too(self):
        source = "def f(xs):\n    for x in xs:\n        y = x\n        print(y)\n    return xs\n"
        assert spans(source) == [(1, 5), (2, 4)]

    def test_a_type_checking_block_is_skipped_and_its_functions_are_not(self):
        source = (
            "from typing import TYPE_CHECKING\n"
            "if TYPE_CHECKING:\n"
            "    def f(x):\n"
            "        y = x\n"
            "        return y\n"
        )
        (only,) = file_blocks(source, "a", "python").blocks
        assert (only.start_line, only.function) == (3, True)

    def test_typing_type_checking_is_skipped_too(self):
        source = "import typing\nif typing.TYPE_CHECKING:\n    a = 1\n    b = 2\n    c = 3\n"
        assert spans(source) == []

    def test_another_condition_is_a_candidate(self):
        source = "if DEBUG:\n    a = 1\n    b = 2\n"
        assert spans(source) == [(1, 3)]

    def test_a_function_block_is_marked_as_a_function(self):
        source = "def f(xs):\n    for x in xs:\n        y = x\n        print(y)\n    return xs\n"
        assert [block.function for block in file_blocks(source, "a", "python").blocks] == [
            True,
            False,
        ]

    def test_a_decorated_function_starts_at_its_def_line(self):
        source = "@cache\ndef f(x):\n    y = x\n    return y\n"
        assert spans(source) == [(2, 4)]


class TestFailures:
    def test_a_file_that_does_not_parse_is_unmeasurable(self):
        with pytest.raises(UnmeasurableSource, match="line 2"):
            file_blocks("x = 1\ndef f(:\n", "a.py", "python")

    def test_a_file_the_tokenizer_rejects_is_unmeasurable(self):
        source = DEDENT_REJECTED
        with pytest.raises(UnmeasurableSource, match="tokenizer rejects"):
            file_blocks(source, "a.py", "python")

    def test_a_language_with_no_table_is_unmeasurable(self):
        with pytest.raises(UnmeasurableSource, match="no rules for language 'json'"):
            file_blocks("{}", "a.json", "json")


class TestGroups:
    def test_a_group_needs_two_instances(self):
        files = [file_blocks(TOTAL, "a.py", "python")]
        assert clone_groups(files) == ()

    def test_instances_across_files_group_and_sort_by_path(self):
        files = [
            file_blocks(TOTAL, "b.py", "python"),
            file_blocks(TOTAL.replace("total", "other"), "a.py", "python"),
        ]
        (group,) = clone_groups(files)
        assert [(block.path, block.start_line) for block in group.instances] == [
            ("a.py", 1),
            ("b.py", 1),
        ]

    def test_clone_lines_are_the_sloc_lines_inside_every_instance(self):
        documented = TOTAL.replace("    out = 0\n", '    """Doc."""\n    out = 0\n')
        files = [file_blocks(TOTAL, "a.py", "python"), file_blocks(documented, "b.py", "python")]
        lines = clone_sloc_lines(files, clone_groups(files))
        assert lines == {"a.py": frozenset({1, 2, 3, 4, 5}), "b.py": frozenset({1, 3, 4, 5, 6})}

    @pytest.mark.parametrize(
        ("source", "first", "second"),
        [
            ("function f(a: number) {\n  const c = a + 1;\n  return c;\n}\n", "a.ts", "b.tsx"),
            ("int f(int a) {\n    int c = a + 1;\n    return c;\n}\n", "a.c", "b.cpp"),
        ],
        ids=["typescript-tsx", "c-cpp"],
    )
    def test_a_copy_between_grammars_of_one_family_groups(self, source, first, second):
        files = [
            file_blocks(source, first, erosion.pair_language(first)),
            file_blocks(source, second, erosion.pair_language(second)),
        ]
        assert len(clone_groups(files)) == 1

    def test_a_javascript_copy_in_typescript_does_not_group(self):
        # The typescript grammar wraps each parameter in `required_parameter`.
        source = "function f(a, b) {\n  const c = a + b;\n  return c;\n}\n"
        files = [
            file_blocks(source, "a.js", "javascript"),
            file_blocks(source, "a.ts", "typescript"),
        ]
        assert clone_groups(files) == ()


class TestHandGroupedFixtures:
    @pytest.mark.parametrize("language", sorted(HAND_GROUPED))
    def test_the_type_two_pair_groups_and_the_near_miss_does_not(self, language):
        name, expected = HAND_GROUPED[language]
        path = FIXTURES / f"complexity_{language}" / name
        files = [file_blocks(path.read_text(encoding="utf-8"), name, language)]

        groups = clone_groups(files)

        assert [
            [(block.start_line, block.end_line) for block in group.instances] for group in groups
        ] == expected
        assert NEAR_MISS_LINE[language] in [block.start_line for block in files[0].blocks]
        grouped = {block.start_line for group in groups for block in group.instances}
        assert NEAR_MISS_LINE[language] not in grouped

    def test_the_hand_goldens_cover_every_language_but_python(self):
        assert sorted(HAND_GROUPED) == sorted(set(CANDIDATES) - {"python"})


def test_clone_key_reads_one_parsed_node():
    tree = grammars.get_parser("python").parse(TOTAL.encode("utf-8"))
    key = clone_key(tree.root_node.children[0], "python")
    assert key.type2 == keys(TOTAL)[0]
    assert len(key.type2) == len(key.exact) == 64


def test_the_minimum_is_three_source_lines():
    assert clones.MIN_SLOC == 3


def fixture_texts():
    for path in sorted(FIXTURES.glob("complexity_*/*")):
        language = TreeSitterExtractor.SUPPORTED_EXTENSIONS.get(path.suffix)
        if path.is_file() and language in CANDIDATES:
            yield pytest.param(path, language, id=f"{path.parent.name}/{path.name}")


def outcome(function, *arguments):
    try:
        return function(*arguments)
    except UnmeasurableSource as exc:
        return ("unmeasurable", str(exc))


def parsed(source, language):
    return grammars.get_parser(language).parse(source.encode("utf-8")).root_node


def tree_form(source, path, language):
    root = parsed(source, language)
    lines = partial(complexity.sloc_line_numbers, root, source, language)
    return file_blocks_tree(root, path, language, lines)


def never_read():
    message = "the tree form read the SLOC lines before it checked the tree"
    raise AssertionError(message)


class TestTheTreeForm:
    @pytest.mark.parametrize(("path", "language"), list(fixture_texts()))
    def test_the_tree_form_equals_the_text_form_on_every_fixture(self, path, language):
        text = path.read_text(encoding="utf-8")

        from_tree = outcome(tree_form, text, path.name, language)

        assert from_tree == outcome(file_blocks, text, path.name, language)

    @pytest.mark.parametrize(
        "source",
        [
            pytest.param("x = 1\ndef f(:\n", id="does-not-parse"),
            pytest.param(DEDENT_REJECTED, id="tokenizer-rejects"),
        ],
    )
    def test_the_tree_form_fails_with_the_text_forms_message(self, source):
        from_tree = outcome(tree_form, source, "a.py", "python")

        assert from_tree[0] == "unmeasurable"
        assert from_tree == outcome(file_blocks, source, "a.py", "python")

    def test_the_tree_form_names_a_language_with_no_table(self):
        with pytest.raises(UnmeasurableSource, match="no rules for language 'json'"):
            file_blocks_tree(parsed(TOTAL, "python"), "a.json", "json", never_read)

    def test_a_tree_that_does_not_parse_is_refused_before_the_lines_are_read(self):
        with pytest.raises(UnmeasurableSource, match="does not parse"):
            file_blocks_tree(parsed("def f(:\n", "python"), "a.py", "python", never_read)

    def test_the_tree_form_never_parses(self, monkeypatch):
        root = parsed(TOTAL, "python")
        expected = file_blocks(TOTAL, "a.py", "python")

        def refuse(_language):
            message = "file_blocks_tree parsed the source again"
            raise AssertionError(message)

        monkeypatch.setattr(grammars, "get_parser", refuse)

        lines = complexity.sloc_line_numbers(root, TOTAL, "python")
        assert file_blocks_tree(root, "a.py", "python", lambda: lines) == expected
