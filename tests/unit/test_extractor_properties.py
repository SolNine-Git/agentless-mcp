"""Nine properties of binding, traversal and identity, pinned before any refactor.

Stage 6 of the remediation moves code that these properties describe. A
characterization test earns its keep only if it was written first, so this
module lands ahead of the refactors and its diffs are what prove what moved.

Four properties hold today and are regression guards. Five do not, and each of
those is a `xfail(strict=True)` naming the finding that fixes it: the marker
states the intent, the suite goes red the day the behaviour changes, and the
person who lands the fix has to remove the marker rather than leave a test
that quietly lies.
"""

import builtins
import time
from collections import Counter
from pathlib import Path

import pytest

from agentless_mcp.core import extractor as extractor_module
from agentless_mcp.core import grammars, refs, resolve, symbols
from agentless_mcp.core.extractor import IdentifierRole, TreeSitterExtractor
from agentless_mcp.core.symbols import ASTSymbol, SymbolKind

FIXTURES = Path(__file__).parent.parent / "characterization" / "fixtures"

# `a` is a package with its own definition, `a.b` a submodule with another.
# Python binds the *package* for `import a.b`, so which file the name `a`
# points at is a question with one right answer.
DOTTED_PACKAGE = {
    "a/__init__.py": "def in_init(value):\n    return value\n",
    "a/b.py": "def in_b(value):\n    return value\n",
    "user.py": "import a.b\n\n\ndef use(value):\n    return a.in_init(value)\n",
    "aliased.py": "import a.b as ab\n\n\ndef use_alias(value):\n    return ab.in_b(value)\n",
}

# `stray.py` imports the module `mod` and then calls a bare `wrapped`, which it
# never imported. Python raises NameError on that line.
SUBMODULE_IMPORT = {
    "pkg/__init__.py": "",
    "pkg/mod.py": "def wrapped(value):\n    return value\n",
    "main.py": "from pkg import mod\n\n\ndef use(value):\n    return mod.wrapped(value)\n",
    "stray.py": "from pkg import mod\n\n\ndef bare(value):\n    return wrapped(value)\n",
}

# One key spelled with a dot, and the same path spelled as nesting. The two
# are different keys in the file and the same qualified name in the id.
DOTTED_YAML = "a:\n  b.c: 1\n  b:\n    c: 2\n"


def yield_symbol(name: str, parent: str, *, line: int) -> ASTSymbol:
    """One extracted symbol, built directly, for an ordering assertion."""
    return ASTSymbol(
        name=name,
        kind=SymbolKind.CONSTANT,
        module_path="c.yaml",
        line_number=line,
        end_line_number=line,
        signature=name,
        docstring="",
        parent_class=parent,
        decorators=(),
        bases=(),
        language="yaml",
        is_public=True,
        is_async=False,
    )


def write(root, files):
    """Write a mapping of relative path to text under ``root``."""
    for relative, text in files.items():
        (root / relative).parent.mkdir(parents=True, exist_ok=True)
        (root / relative).write_text(text, encoding="utf-8")
    return root


def scopes_for(root, extractor):
    """Every file's resolved import scope."""
    return resolve.build_file_scopes(refs.scan_repo(root, extractor).files)


def named(collection):
    """Map symbol name to symbol, for assertions that read as prose."""
    return {symbol.name: symbol for symbol in collection}


class TestDottedImportBinding:
    """What name a dotted `import` introduces, and what file it points at."""

    def test_import_a_dot_b_binds_the_package_not_the_submodule(self, tmp_path, extractor):
        """Was a strict xfail; the marker came off when stage 6c landed.

        `build_file_scopes` bound `module.split(".")[0]` to the target of the
        whole dotted path, so `import a.b` bound `a` to a/b.py -- and
        `a.in_init()` was attributed to the one file that does not define it.
        """
        scope = scopes_for(write(tmp_path, DOTTED_PACKAGE), extractor)["user.py"]
        assert scope.module_bindings["a"] == frozenset({"a/__init__.py"})

    def test_an_aliased_dotted_import_binds_the_alias_alone(self, tmp_path, extractor):
        """Was a strict xfail; the marker came off when stage 6c landed.

        The extractor dropped the alias, so the binding introduced was `a` --
        a name the file never binds -- and `ab`, the name it does bind, had
        none.
        """
        scope = scopes_for(write(tmp_path, DOTTED_PACKAGE), extractor)["aliased.py"]
        assert scope.module_bindings["ab"] == frozenset({"a/b.py"})
        assert "a" not in scope.module_bindings

    def test_a_dotted_module_string_is_not_truncated_to_its_first_segment(self, extractor):
        # `_extract_import_path` kept only the first of a run of sibling
        # identifiers, so `import pricing.Money` recorded module='pricing' and
        # the import resolved to a package rather than to the file that
        # defines Money. C#'s `using App.Money` had the same shape.
        source = (FIXTURES / "tier2" / "Pricing.scala").read_text(encoding="utf-8")
        statements = extractor.extract_imports_from_source(source, "scala", "P.scala")
        assert "pricing.Money" in [statement.module for statement in statements]

    def test_a_csharp_using_directive_keeps_its_whole_path(self, extractor):
        # The same run-of-siblings shape as Scala, one level deeper: the path
        # is under a `qualified_name`, and only its first segment survived.
        statements = extractor.extract_imports_from_source("using App.Money;\n", "csharp", "C.cs")
        assert [statement.module for statement in statements] == ["App.Money"]

    def test_a_from_import_alias_binds_the_alias(self, tmp_path, extractor):
        # `from pkg import mod as m` binds one name here and it is `m`.
        # Resolution still needs `mod`, which is why the statement carries
        # both.
        source = "from pkg import mod as m\n\n\ndef use(v):\n    return m.wrapped(v)\n"
        (statement,) = extractor.extract_imports_from_source(source, "python", "a.py")
        assert statement.bound_names() == (("mod", "m"),)


class TestAsyncDetection:
    """`is_async` is persisted to the tag cache and rendered into signatures."""

    @pytest.mark.parametrize(
        "source", ["def async_handler(x): pass\n", "def asynchronous(x): pass\n"]
    )
    def test_a_function_merely_named_async_is_not_async(self, extractor, source):
        """Was a strict xfail; the marker came off when stage 6c landed.

        The Python handler substring-matched 'async' in the first bytes of the
        declaration, so both `def async_handler(x)` and `def asynchronous(x)`
        came back is_async=True with a rendered signature of `async def ...`.
        The Rust half of the same defect is pinned in
        tests/unit/test_tier1_languages.py.
        """
        (symbol,) = extractor.extract_from_source(source, "python", "a.py")
        assert symbol.is_async is False
        assert symbol.signature.startswith("def ")

    @pytest.mark.parametrize(
        ("language", "source", "expected"),
        [
            ("javascript", "async function f(){}\nfunction h(){}\n", {"f": True, "h": False}),
            ("javascript", "const g = async () => {};\n", {"g": True}),
            ("csharp", "class C { async Task M() {} void N() {} }\n", {"M": True, "N": False}),
        ],
    )
    def test_the_generic_walker_reads_async_from_the_declaration(
        self, extractor, language, source, expected
    ):
        """The generic walker hardcoded is_async=False for all sixteen languages.

        Three spellings reach it: a keyword child of the declaration, a
        keyword child of the value a binding names, and a modifier keyword
        beside the visibility one. Under-reporting is not neutral here --
        `is_async` is persisted and rendered, so a caller filtering on it gets
        a short answer presented as a complete one.
        """
        found = named(extractor.extract_from_source(source, language, "a"))
        assert {name: found[name].is_async for name in expected} == expected

    def test_a_genuinely_async_function_is_async(self, extractor):
        # Positive control, so the xfail above reads as over-matching rather
        # than as async detection being absent.
        (symbol,) = extractor.extract_from_source("async def real(x): pass\n", "python", "a.py")
        assert symbol.is_async is True


class TestConditionalImports:
    def test_a_type_checking_import_reaches_the_import_graph(self, extractor):
        """Was a strict xfail; the marker came off when stage 6c landed.

        The Python import walk iterated root children only, so the source
        below yielded one import, `typing`, and none for `foo` -- and a
        type-checking block is where a repository puts exactly the imports
        that would otherwise be cycles.
        """
        source = "from typing import TYPE_CHECKING\n\nif TYPE_CHECKING:\n    from foo import Bar\n"
        statements = extractor.extract_imports_from_source(source, "python", "a.py")
        modules = [statement.module for statement in statements]
        assert "foo" in modules


class TestWholeModuleEvidence:
    """Importing a module must not make everything inside it look imported."""

    def test_importing_a_submodule_binds_its_name(self, tmp_path, extractor):
        scope = scopes_for(write(tmp_path, SUBMODULE_IMPORT), extractor)["main.py"]
        assert scope.named["mod"] == frozenset({"pkg/mod.py"})

    def test_a_name_the_file_never_imported_is_not_imported_evidence(self, tmp_path, extractor):
        """Was a strict xfail; the marker came off when stage 6b landed.

        `build_file_scopes` added a `from pkg import mod` target to the
        whole-module set, so every symbol `pkg/mod.py` defines counted as
        imported into the importing file. stray.py below calls a bare
        `wrapped` it never imported -- a NameError in Python -- and the
        resolver answered `imported`, the tier an agent is told to read as a
        caller.
        """
        root = write(tmp_path, SUBMODULE_IMPORT)
        scan = refs.scan_repo(root, extractor)
        resolver, _ = resolve.resolve_repo(scan, refs.build_ref_index(scan))

        resolution = resolver.resolve("wrapped", "stray.py")
        assert resolution is None or resolution.tier is not resolve.Tier.IMPORTED


class TestTypeScriptNamedImports:
    def test_a_named_import_records_the_names_it_binds(self, extractor):
        """Was a strict xfail; the marker came off when stage 6b landed.

        While the names were dropped, a TypeScript named import could only
        reach the resolver through its whole-module set -- which is why that
        set was allowed to supply bare-name evidence for every language,
        including the ones where importing a module binds nothing of the sort.
        """
        source = 'import { X } from "./m";\n'
        (statement,) = extractor.extract_imports_from_source(source, "typescript", "a.ts")
        assert statement.names == ("X",)

    def test_the_local_name_is_what_is_recorded(self, extractor):
        # `import { Y as Z }` binds Z. What a bare reference in this file can
        # spell is the question, and Y is not it.
        source = 'import { Y as Z } from "./m";\n'
        (statement,) = extractor.extract_imports_from_source(source, "typescript", "a.ts")
        assert statement.names == ("Z",)

    def test_a_namespace_import_binds_no_bare_name(self, extractor):
        # `import * as ns` binds a module object, the same as Go's
        # `import "fmt"` and Python's `import a.b`. A bare reference to
        # something the module defines is not evidence of an import.
        source = 'import * as ns from "./n";\n'
        (statement,) = extractor.extract_imports_from_source(source, "typescript", "a.ts")
        assert statement.names == ()


class TestTypeOwnership:
    """A method carries a parent; the parent has to exist as a symbol."""

    def test_go_methods_name_their_receiver_type(self, extractor):
        source = (
            "package p\n\ntype Invoice struct {\n\tSubtotal float64\n}\n\n"
            "func (i *Invoice) Price() float64 { return 0 }\n"
        )
        found = named(extractor.extract_from_source(source, "go", "p.go"))
        assert found["Price"].parent_class == "Invoice"

    def test_a_go_struct_type_is_a_symbol_of_its_own(self, extractor):
        """Was a strict xfail; the marker came off when stage 6c landed.

        Go's class node type was `type_declaration`, which names nothing --
        so it matched, yielded no symbol, and stopped the descent before
        reaching the `type_spec` that carries the name. The source below used
        to yield one symbol, the method, and none for Invoice.
        """
        source = (
            "package p\n\ntype Invoice struct {\n\tSubtotal float64\n}\n\n"
            "func (i *Invoice) Price() float64 { return 0 }\n"
        )
        found = named(extractor.extract_from_source(source, "go", "p.go"))
        assert found["Invoice"].kind is SymbolKind.CLASS


class TestFunctionValuedBindings:
    """The dominant way modern JS and TS declare a function."""

    def test_a_function_declaration_is_extracted(self, extractor):
        # Positive control: the `function` keyword form does arrive, so the
        # xfail below is about the assigned form, not about the grammar.
        for language in ("javascript", "typescript", "tsx"):
            found = named(extractor.extract_from_source("function h(){}\n", language, "a.js"))
            assert found["h"].kind is SymbolKind.FUNCTION

    @pytest.mark.parametrize("language", ["javascript", "typescript", "tsx"])
    def test_a_const_bound_function_is_extracted(self, extractor, language):
        """Was a strict xfail; the marker came off when stage 6c landed.

        The ECMAScript walker matched `function_declaration` and
        `class_declaration` and not a binding whose value is a function, so
        `export const App = () => {}` and `const g = function(){}` yielded no
        symbols in any of the three grammars -- and that is the dominant way
        modern JS and TS declare a function.
        """
        source = "export const App = () => { return 1; };\nconst g = function(){ return 2; };\n"
        found = named(extractor.extract_from_source(source, language, "a.js"))
        assert "App" in found
        assert "g" in found


class TestStableIdentity:
    """One symbol, one id -- the contract an agent holds across a session."""

    def test_every_committed_fixture_mints_distinct_ids(self, extractor):
        extensions = TreeSitterExtractor.SUPPORTED_EXTENSIONS
        checked = 0
        for path in sorted(FIXTURES.rglob("*")):
            if not path.is_file() or path.suffix not in extensions:
                continue
            relative = str(path.relative_to(FIXTURES))
            extracted = extractor.extract_from_source(
                path.read_text(encoding="utf-8"), extensions[path.suffix], relative
            )
            if not extracted:
                continue
            checked += 1
            minted = [symbols.symbol_stable_id(s) for s in symbols.disambiguate(extracted)]
            assert len(set(minted)) == len(minted), f"{relative} mints a duplicate id"
        assert checked, "no fixture was checked -- the extension filter is wrong"

    def test_a_dotted_key_cannot_collide_with_a_nested_one(self, extractor):
        """Was a strict xfail; the marker came off when stage 6a landed.

        `disambiguate` counted on `(parent_class, name)` -- the pair the
        qualified name is built from -- while the id spells the qualified
        name. The pair keeps the dot's position and the name does not, so a
        key `b.c` under parent `a` and a key `c` under parent `a.b` were two
        different pairs and one id, and neither was renumbered.
        """
        extracted = extractor.extract_from_source(DOTTED_YAML, "yaml", "c.yaml")
        minted = [symbols.symbol_stable_id(s) for s in symbols.disambiguate(extracted)]

        assert len(extracted) == 4
        assert Counter(minted).most_common(1)[0][1] == 1

    def test_the_renumbered_one_is_the_later_of_the_two(self):
        # The ordinal follows the source: the first symbol at a qualified name
        # keeps it and the later one carries `#2`, so inserting a key above
        # another does not renumber the one a caller already holds.
        minted = [
            symbols.symbol_stable_id(s)
            for s in symbols.disambiguate(
                (yield_symbol("b.c", "a", line=2), yield_symbol("c", "a.b", line=4))
            )
        ]
        assert minted == ["yaml:c.yaml::a.b.c", "yaml:c.yaml::a.b.c#2"]

    def test_an_id_that_named_one_symbol_is_unchanged(self, extractor):
        """The migration question, answered by measurement rather than policy.

        Measured across this repository: 4824 symbols, 0 ids changed, 0 files
        minting a duplicate. The fix only moves an id that already named two
        symbols -- so a caller holding a pre-fix id either holds one that
        still resolves, or holds one that was ambiguous when they got it.
        There is no spelling to keep resolving for a release.
        """
        source = "class Book:\n    def price(self):\n        return 1\n"
        (book, price) = symbols.disambiguate(
            extractor.extract_from_source(source, "python", "core.py")
        )
        assert symbols.symbol_stable_id(book) == "py:core.py::Book"
        assert symbols.symbol_stable_id(price) == "py:core.py::Book.price"


# A left-nested chain: `a + a + ... + a` parses to a binary_operator tree whose
# depth is the number of terms, so the parse tree of a 6 KB file is 1500 nodes
# deep. Every shape that reaches this depth is ordinary Python -- a long `|`
# type union, a long `or` guard, a generated concatenation.
CHAIN_TERMS = 3000
CHAINED_EXPRESSION = "x = " + " + ".join(["a"] * CHAIN_TERMS) + "\n"

# Generous enough that a loaded CI box does not flake, and two orders of
# magnitude below the cost of the walk this pins against. The parent-chain
# walk took 288 seconds on a 20 KB file of this shape; the scope-boundary
# index takes 0.02.
CHAIN_BUDGET_SECONDS = 2.0


class TestReferencePassCostIsLinear:
    """The reference pass must not cost the depth of an expression.

    Classifying a Python identifier needs the scopes that enclose it. Asking
    for them by walking the node's parent chain costs the node's depth, and a
    chained expression makes depth equal to file size -- so the pass went
    cubic in the length of one line and a 20 KB file took 288 seconds, against
    0.044 for the whole-scope scan it had replaced. `_ScopeTree` answers from a
    boundary index built in one downward walk, which costs the number of
    enclosing *scopes* instead.

    Wall-clock rather than a call count, because the failure is a hang: a
    repository holding one generated file stopped being indexable at all, and
    no assertion about the shape of the traversal would have said so.
    """

    def test_a_chained_expression_is_classified_in_linear_time(self, extractor):
        started = time.perf_counter()
        found = extractor.extract_refs_from_source(CHAINED_EXPRESSION, "python", "chain.py")
        elapsed = time.perf_counter() - started

        assert len(found) == CHAIN_TERMS + 1
        assert elapsed < CHAIN_BUDGET_SECONDS

    def test_depth_costs_no_more_than_breadth(self, extractor):
        """The same identifiers, nested and flat, cost the same order.

        The flat form was never slow, so comparing the two is what separates
        "this machine is loaded" from "depth is being paid for". A ratio bound
        rather than an equality: the deep form does more real work, just not
        more per level of nesting.
        """
        flat = "x = [" + ", ".join(["a"] * CHAIN_TERMS) + "]\n"

        started = time.perf_counter()
        extractor.extract_refs_from_source(flat, "python", "flat.py")
        flat_seconds = time.perf_counter() - started

        started = time.perf_counter()
        extractor.extract_refs_from_source(CHAINED_EXPRESSION, "python", "chain.py")
        chained_seconds = time.perf_counter() - started

        assert chained_seconds < max(flat_seconds * 20.0, 0.5)

    def test_a_comprehension_still_hides_its_bindings_from_its_first_iterable(self, extractor):
        """The correctness the parent walk bought, kept by the index.

        `rows` is the comprehension's own binding and `source` is not, so a
        name in the first iterable sees the enclosing scope and a name in the
        body sees the comprehension. The index carries the bounding node for
        exactly this test.
        """
        source = "def use(source):\n    return [rows for rows in source if rows]\n"
        roles = {
            (found.name, found.line): found.role
            for found in extractor.extract_refs_from_source(source, "python", "c.py")
        }

        assert roles[("source", 2)] is IdentifierRole.LOCAL
        assert roles[("rows", 2)] is IdentifierRole.LOCAL


VISIBILITY_SOURCES = {
    "typescript": (
        "s.ts",
        (
            "export class Svc {\n"
            "    private secret(): number { return 1; }\n"
            "    public open(): number { return 2; }\n"
            "}\n"
        ),
    ),
    "swift": (
        "s.swift",
        (
            "class Svc {\n"
            "    private func secret() -> Int { return 1 }\n"
            "    func open() -> Int { return 2 }\n"
            "}\n"
        ),
    ),
}


class TestVisibilityKeywordsBeatTheUnderscoreConvention:
    """`is_public` is a persisted column, so a wrong one is wrong on disk.

    `_generic_is_public` reads the declaration's own keywords where its row
    lists them and falls back to the leading-underscore rule where it does
    not. TypeScript spells the keyword `accessibility_modifier` and Swift
    wraps it in `modifiers`, and neither row listed one -- so `private
    secret()` came back public in the two languages where the keyword is most
    often written.
    """

    @pytest.mark.parametrize("language", sorted(VISIBILITY_SOURCES))
    def test_a_private_member_is_not_reported_public(self, language, extractor):
        if language not in grammars.warmed_languages():
            pytest.skip(f"grammar for {language} is not in the local pack cache")
        path, source = VISIBILITY_SOURCES[language]
        found = {s.name: s for s in extractor.extract_from_source(source, language, path)}

        assert found["secret"].is_public is False
        assert found["open"].is_public is True


class TestRationaleLinesUseTheRowGrammarTreeSitterUses:
    """A comment's rows are its newlines, and nothing else.

    `str.splitlines` also breaks on a form feed, a vertical tab, U+0085 and
    U+2028; tree-sitter counts rows on `\n` alone. A form feed is the Emacs
    page marker and is ordinary in older C sources, and one inside a block
    comment moved every later marker in that comment down a line --
    `_attach_rationales` then hangs it off whichever symbol that line falls in.
    """

    def test_a_form_feed_inside_a_comment_does_not_move_the_line(self, extractor):
        if "c" not in grammars.warmed_languages():
            pytest.skip("grammar for c is not in the local pack cache")
        source = "/* pad\n\x0cmore\nTODO: fix me */\nint x;\n"
        parser = grammars.get_parser("c")
        data = source.encode("utf-8")

        found = extractor_module._extract_rationales(parser.parse(data).root_node, data)

        assert [rationale.line_number for rationale in found] == [3]


class TestTheAlwaysBoundNamesDoNotMoveWithTheInterpreter:
    """The same tree must classify the same way on every supported Python.

    The set was `dir(builtins)` of the running interpreter, and the tag cache
    generation is keyed on the tree rather than on the interpreter -- so a
    repository indexed under 3.10 and served under 3.13 disagreed about
    `ExceptionGroup` and four other names, from one cache.
    """

    def test_the_set_is_a_literal_and_not_the_running_interpreter(self):
        # The names CPython added after the 3.10 floor. They must be present
        # whatever interpreter runs this, which is only true of a literal.
        assert {"ExceptionGroup", "BaseExceptionGroup", "aiter", "anext"} <= (
            extractor_module._PYTHON_ALWAYS_BOUND
        )
        # And the pin must not have drifted from the language: every name this
        # interpreter calls a builtin is in it.
        assert set(dir(builtins)) <= extractor_module._PYTHON_ALWAYS_BOUND

    def test_a_soft_keyword_is_still_an_ordinary_name(self):
        # `match` and `case` are names everywhere except one statement, and a
        # repository function called `match` has callers.
        assert "match" not in extractor_module._PYTHON_ALWAYS_BOUND
        assert "case" not in extractor_module._PYTHON_ALWAYS_BOUND
