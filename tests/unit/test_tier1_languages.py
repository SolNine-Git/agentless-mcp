"""The tier-1 languages no fixture reached: Rust, C, C++ and Java.

The audit found `core/extractor.py` at 82% line / 67% branch, the weakest in
the package, and found why: the committed fixtures spell py/ts/go and the ten
tier-2 languages, and there was no `.rs`, `.c`, `.cpp`, `.h` or `.java` file
anywhere. Two hand-written handlers and one generic-walker language ran
against zero fixtures, so their behaviour was whatever the code happened to
do. The names pair with `test_tier2_languages.py`, which does the same job
for the ten languages below these.

These tests pin what it does today, including where that is wrong. A property
this module knows to be broken is a `xfail(strict=True)`: it states the
intended behaviour, it fails now, and it turns the suite red the day a fix
lands so nobody can quietly leave the marker behind.

Grammar-dependent by construction, like `test_tier2_languages.py`: a language
the local pack has never downloaded is a cold cache, not a failure.
"""

from pathlib import Path

import pytest

from agentless_mcp.core import grammars
from agentless_mcp.core.symbols import SymbolKind

FIXTURES = Path(__file__).parent.parent / "characterization" / "fixtures" / "tier1"

# All four spell the same tiny pricing module the tier-2 fixtures spell, so a
# handler that extracts nothing is visible as a missing symbol rather than as
# a differently shaped file.
FIXTURE_FILES = {
    "rust": "pricing.rs",
    "c": "pricing.c",
    "c_header": "pricing.h",
    "cpp": "pricing.cpp",
    "java": "Pricing.java",
}

GRAMMAR_FOR = {"rust": "rust", "c": "c", "c_header": "c", "cpp": "cpp", "java": "java"}


def symbols_for(extractor, fixture: str):
    """Extract one fixture's symbols, skipping when its grammar is cold."""
    grammar = GRAMMAR_FOR[fixture]
    if grammar not in grammars.warmed_languages():
        pytest.skip(
            f"grammar for {grammar} is not in the local pack cache: run agentless-mcp warmup"
        )
    name = FIXTURE_FILES[fixture]
    source = (FIXTURES / name).read_text(encoding="utf-8")
    return extractor.extract_from_source(source, grammar, name)


def imports_for(extractor, fixture: str):
    """Extract one fixture's imports, skipping when its grammar is cold."""
    grammar = GRAMMAR_FOR[fixture]
    if grammar not in grammars.warmed_languages():
        pytest.skip(
            f"grammar for {grammar} is not in the local pack cache: run agentless-mcp warmup"
        )
    name = FIXTURE_FILES[fixture]
    source = (FIXTURES / name).read_text(encoding="utf-8")
    return extractor.extract_imports_from_source(source, grammar, name)


def require_grammar(grammar: str) -> None:
    """Skip when ``grammar`` is cold, for a test whose source is inline.

    The same policy the fixture helpers apply, spelled once for the tests that
    hold their source in the module rather than in a file.
    """
    if grammar not in grammars.warmed_languages():
        pytest.skip(
            f"grammar for {grammar} is not in the local pack cache: run agentless-mcp warmup"
        )


def named(symbols):
    """Map symbol name to symbol, for assertions that read as prose."""
    return {symbol.name: symbol for symbol in symbols}


class TestRustTopLevelItems:
    """What `_visit_rust_item` dispatches on, one arm at a time."""

    def test_each_item_kind_reaches_its_own_symbol_kind(self, extractor):
        found = named(symbols_for(extractor, "rust"))
        assert found["TAX_RATE"].kind is SymbolKind.CONSTANT
        assert found["Money"].kind is SymbolKind.TYPE_ALIAS
        assert found["Priceable"].kind is SymbolKind.PROTOCOL
        assert found["Invoice"].kind is SymbolKind.CLASS
        assert found["Status"].kind is SymbolKind.ENUM
        assert found["apply_tax"].kind is SymbolKind.FUNCTION

    def test_a_signature_carries_the_keyword_that_declared_it(self, extractor):
        found = named(symbols_for(extractor, "rust"))
        assert found["Invoice"].signature == "struct Invoice"
        assert found["Status"].signature == "enum Status"
        assert found["Priceable"].signature == "trait Priceable"
        assert found["apply_tax"].signature == "fn apply_tax(amount: Money, rate: f64) -> Money"

    def test_impl_methods_carry_the_type_as_their_parent(self, extractor):
        found = named(symbols_for(extractor, "rust"))
        assert found["new"].kind is SymbolKind.METHOD
        assert found["new"].parent_class == "Invoice"
        assert found["price"].parent_class == "Invoice"

    def test_visibility_is_read_off_the_modifier_not_the_name(self, extractor):
        # `pub` on every item in the fixture, so a handler that never looks
        # would report the same answer -- the point is that it looks.
        assert all(symbol.is_public for symbol in symbols_for(extractor, "rust"))

    def test_use_declarations_become_imports(self, extractor):
        found = {statement.module: statement for statement in imports_for(extractor, "rust")}
        assert found["std::collections::HashMap"].is_relative is False
        assert found["crate::money::Currency"].is_relative is True


class TestRustGaps:
    """Rust properties that root-children-only traversal used to break."""

    def test_a_function_inside_a_module_is_extracted(self, extractor):
        """Was a strict xfail; the marker came off when stage 6c landed.

        `_extract_rust_symbols` iterated the root's direct children, so a
        `mod_item`'s contents were never visited and pricing.rs yielded 8
        symbols with `round_half_up` not among them.
        """
        assert "round_half_up" in named(symbols_for(extractor, "rust"))

    def test_an_async_method_is_reported_async(self, extractor):
        """Was a strict xfail; the marker came off when stage 6c landed.

        `_extract_rust_function` tested for an `async` child of the
        `function_item`, and tree-sitter-rust nests `async` inside a
        `function_modifiers` node: `pub async fn price` reported
        is_async=False, and no Rust function had ever been reported async.
        """
        price = named(symbols_for(extractor, "rust"))["price"]
        assert price.is_async
        assert price.signature.startswith("async fn ")

    def test_a_cfg_test_module_contributes_its_items(self, extractor):
        """Was a strict xfail; the marker came off when stage 6c landed.

        `#[cfg(test)] mod tests { use super::*; }` is how every Rust crate
        writes its tests, and the block below used to yield zero symbols and
        zero imports -- invisible to both the symbol map and the import graph.
        """
        source = "#[cfg(test)]\nmod tests {\n    use super::*;\n    pub fn helper() {}\n}\n"
        extracted = extractor.extract_from_source(source, "rust", "a.rs")
        imported = extractor.extract_imports_from_source(source, "rust", "a.rs")
        assert [symbol.name for symbol in extracted] == ["helper"]
        assert [statement.module for statement in imported] == ["super::*"]


class TestCTranslationUnit:
    """What the C handler sees in an unguarded `.c` file."""

    def test_functions_and_tagged_types_are_extracted(self, extractor):
        found = named(symbols_for(extractor, "c"))
        assert found["apply_tax"].kind is SymbolKind.FUNCTION
        assert found["Invoice"].signature == "struct Invoice"
        assert found["Status"].signature == "enum Status"

    def test_an_enum_is_reported_as_a_class(self, extractor):
        # Pinned as-is, and it disagrees with Rust, where enum_item maps to
        # SymbolKind.ENUM. One of the two is wrong; this records which.
        assert named(symbols_for(extractor, "c"))["Status"].kind is SymbolKind.CLASS

    def test_a_pointer_returning_function_keeps_its_own_name(self, extractor):
        # `double *invoice_price(...)` nests the identifier under a
        # pointer_declarator; the name must not come back as the pointer.
        assert "invoice_price" in named(symbols_for(extractor, "c"))

    def test_includes_split_on_the_bracket_form(self, extractor):
        found = {statement.module: statement for statement in imports_for(extractor, "c")}
        assert found["stdio.h"].is_relative is False
        assert found["money.h"].is_relative is True


class TestCHeaderGuard:
    """The include guard is the normal shape of a C header, and it hides everything."""

    def test_a_guarded_header_yields_its_declarations(self, extractor):
        """Was a strict xfail; the marker came off when stage 6c landed.

        `#ifndef` wraps a whole translation unit in one `preproc_ifdef`, so a
        guarded header's symbols sit one level too deep for a root-children
        walk. pricing.h used to yield 0 symbols, and nearly every C header in
        a real repository is guarded.
        """
        found = named(symbols_for(extractor, "c_header"))
        assert "Money" in found
        assert "apply_tax_inline" in found

    def test_a_guarded_header_yields_its_includes(self, extractor):
        # The import half of the same defect: a guarded header contributed no
        # edges to the import graph either.
        assert [statement.module for statement in imports_for(extractor, "c_header")] == ["money.h"]


class TestCPrototypes:
    """A header of prototypes is the normal shape of a C header."""

    def test_a_prototype_yields_a_function_symbol(self, extractor):
        # A prototype is a `declaration`, not a `function_definition`, so the
        # declarations a translation unit resolves against used to be the ones
        # with no symbol at all.
        source = "double apply_tax(double a, double r);\ndouble *scaled(double a);\n"
        found = named(extractor.extract_from_source(source, "c", "money.h"))
        assert found["apply_tax"].kind is SymbolKind.FUNCTION
        assert found["scaled"].kind is SymbolKind.FUNCTION

    def test_a_function_pointer_variable_is_not_a_function(self, extractor):
        # `int (*fp)(void);` declares a variable. Its declarator reaches a
        # name only through a parenthesis and a star, which is the
        # function-pointer shape, and a symbol map is read as code.
        source = "int (*fp)(void);\n"
        assert extractor.extract_from_source(source, "c", "money.h") == []

    def test_a_function_returning_a_function_pointer_is_still_a_function(self, extractor):
        # The other side of the same shape: `int (*make(void))(int);` wraps a
        # further `function_declarator`, and that inner one is the declaration
        # the file makes. Refusing the whole shape would lose it.
        source = "int (*make(void))(int);\n"
        assert "make" in named(extractor.extract_from_source(source, "c", "money.h"))

    def test_a_tagged_type_inside_a_declaration_still_arrives(self, extractor):
        # The prototype match is on the declarator rather than on the
        # declaration, so a `declaration` holding a tagged type is still
        # descended into.
        source = "struct Money { double amount; } m;\n"
        assert "Money" in named(extractor.extract_from_source(source, "c", "money.h"))


# A routine C++ source file: the class is declared in a header and its methods
# are defined out of line here. Four of the five function definitions below
# spell a name that is not a Python identifier.
CPP_MEMBERS = """\
namespace acme {

Logger::Logger() : level_(0) {}

Logger::~Logger() {}

int Logger::emit(int x) { return x + level_; }

bool operator==(const Logger& a, const Logger& b) { return true; }

int free_function(int y) { return y; }

}  // namespace acme
"""

CPP_INLINE_CLASS = """\
class Logger {
  public:
    int emit(int x) { return x; }
    void reset();
    int level_;
};
"""


class TestCppMemberDefinitions:
    """The forms a C++ source file is actually written in.

    `name.isidentifier()` was the test for "this declarator names a function".
    It is a proxy for the shape, and it refused every C++ spelling that is not
    a bare Python identifier, so the file above yielded one symbol out of five
    -- and `Logger::emit` had been a symbol before the guard landed.
    """

    def test_every_definition_in_a_routine_source_file_is_a_symbol(self, extractor):
        require_grammar("cpp")
        found = named(extractor.extract_from_source(CPP_MEMBERS, "cpp", "logger.cpp"))

        assert set(found) == {"Logger", "~Logger", "emit", "operator==", "free_function"}

    def test_an_out_of_line_definition_carries_the_class_it_names(self, extractor):
        require_grammar("cpp")
        found = named(extractor.extract_from_source(CPP_MEMBERS, "cpp", "logger.cpp"))

        assert found["emit"].parent_class == "Logger"
        assert found["emit"].kind is SymbolKind.METHOD

    def test_a_constructor_and_a_destructor_are_both_extracted(self, extractor):
        require_grammar("cpp")
        found = named(extractor.extract_from_source(CPP_MEMBERS, "cpp", "logger.cpp"))

        assert found["Logger"].parent_class == "Logger"
        assert found["~Logger"].parent_class == "Logger"

    def test_a_free_operator_is_not_owned_by_a_class(self, extractor):
        require_grammar("cpp")
        found = named(extractor.extract_from_source(CPP_MEMBERS, "cpp", "logger.cpp"))

        assert found["operator=="].parent_class == ""
        assert found["operator=="].kind is SymbolKind.FUNCTION

    def test_a_nested_qualified_definition_keeps_every_scope(self, extractor):
        require_grammar("cpp")
        source = "int A::B::method(int x) { return x; }\n"
        found = named(extractor.extract_from_source(source, "cpp", "q.cpp"))

        assert found["method"].parent_class == "A.B"

    def test_a_template_class_owns_its_method_without_its_arguments(self, extractor):
        require_grammar("cpp")
        source = "template<class T> int C<T>::go() { return 1; }\n"
        found = named(extractor.extract_from_source(source, "cpp", "t.cpp"))

        assert found["go"].parent_class == "C"

    def test_a_method_declared_inside_a_class_body_is_a_symbol(self, extractor):
        """A `class_specifier` ends one descent and opens another.

        The class used to be the end of the walk, so an inline class -- the
        whole surface of a header-only library -- contributed the class name
        and nothing it declares.
        """
        require_grammar("cpp")
        found = named(extractor.extract_from_source(CPP_INLINE_CLASS, "cpp", "logger.hpp"))

        assert found["emit"].parent_class == "Logger"
        assert found["reset"].parent_class == "Logger"

    def test_a_data_member_is_not_reported_as_a_function(self, extractor):
        # `int level_;` is a `field_declaration` too, and it ends the descent
        # for the same reason a method does. Only the one whose declarator is
        # a `function_declarator` becomes a symbol.
        require_grammar("cpp")
        found = named(extractor.extract_from_source(CPP_INLINE_CLASS, "cpp", "logger.hpp"))

        assert "level_" not in found


class TestCppNamespace:
    def test_a_declaration_outside_any_namespace_is_extracted(self, extractor):
        # The positive control: the same file's root-level members do arrive,
        # so the namespace test below is about depth, not about C++ support.
        found = named(symbols_for(extractor, "cpp"))
        assert found["Ledger"].signature == "class Ledger"
        assert found["top_level_total"].kind is SymbolKind.FUNCTION

    def test_a_declaration_inside_a_namespace_is_extracted(self, extractor):
        """Was a strict xfail; the marker came off when stage 6c landed.

        A C++ `namespace_definition` is a root child whose members are one
        level deeper, and the C handler never descended: pricing.cpp used to
        yield 2 symbols, with neither Invoice nor apply_tax among them.
        """
        found = named(symbols_for(extractor, "cpp"))
        assert "Invoice" in found
        assert "apply_tax" in found


class TestJava:
    """Tier-1 by the grammar table, and reached by the generic walker."""

    def test_types_and_their_members_are_extracted(self, extractor):
        extracted = symbols_for(extractor, "java")
        found = named(extracted)
        # A constructor carries its own class's name, so the two share a key.
        # Selecting on kind rather than on name is the honest lookup here.
        types = [s for s in extracted if s.kind is SymbolKind.CLASS and s.name == "Invoice"]

        assert [s.signature for s in types] == ["class Invoice"]
        assert found["applyTax"].parent_class == "Invoice"
        assert found["applyTax"].kind is SymbolKind.METHOD

    def test_a_package_import_keeps_its_whole_dotted_path(self, extractor):
        # Java is the one generic-walker language whose dotted import survives
        # intact, which is what makes the Scala truncation in
        # test_extractor_properties.py a handler difference rather than a rule.
        assert [s.module for s in imports_for(extractor, "java")] == ["app.money.Currency"]

    def test_an_interface_is_reported_as_a_class(self, extractor):
        # Pinned as-is. Rust's `trait` maps to SymbolKind.PROTOCOL and Java's
        # `interface` -- the same idea -- maps to CLASS, so the kind an agent
        # reads depends on which language the file happens to be in.
        assert named(symbols_for(extractor, "java"))["Priceable"].kind is SymbolKind.CLASS

    def test_a_private_method_is_not_reported_public(self, extractor):
        """Was a strict xfail; the marker came off when stage 6c landed.

        The generic walker answered `is_public` from the leading-underscore
        convention for all sixteen of its languages, so `private void
        hidden()` came back is_public=True -- and the value is persisted to
        the tag cache, so the column was wrong on disk too.
        """
        source = "class C {\n    private void hidden() {}\n    public void shown() {}\n}\n"
        found = named(extractor.extract_from_source(source, "java", "C.java"))
        assert found["hidden"].is_public is False
        assert found["shown"].is_public is True

    def test_an_instance_field_is_not_a_constant(self, extractor):
        # `private final double subtotal` is per-instance state that happens
        # not to be reassigned. `static final` is Java's constant, and the
        # difference is a guarantee the map would otherwise claim falsely.
        found = named(symbols_for(extractor, "java"))
        assert "subtotal" not in found

    def test_a_static_final_field_is_a_constant(self, extractor):
        # The generic walker had no constant branch at all, so
        # `static final double TAX_RATE = 0.2` yielded no symbol and the
        # fixture's only constant was invisible.
        assert named(symbols_for(extractor, "java"))["TAX_RATE"].kind is SymbolKind.CONSTANT
