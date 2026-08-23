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
    """Two Rust properties that do not hold today."""

    @pytest.mark.xfail(
        strict=True,
        reason=(
            "B05-H4: _extract_rust_symbols iterates root children only, so a mod_item's "
            "contents are never visited. Measured: pricing.rs yields 8 symbols and "
            "round_half_up is not among them. Fixed by stage 6c."
        ),
    )
    def test_a_function_inside_a_module_is_extracted(self, extractor):
        assert "round_half_up" in named(symbols_for(extractor, "rust"))

    @pytest.mark.xfail(
        strict=True,
        reason=(
            "B05-H7: _extract_rust_function tests `any(c.type == 'async' ...)` on the "
            "function_item's direct children, but tree-sitter-rust nests `async` inside a "
            "`function_modifiers` node. Measured: `pub async fn price` reports "
            "is_async=False and signature 'fn price(...)'. No Rust function has ever been "
            "reported async. Fixed by stage 6c."
        ),
    )
    def test_an_async_method_is_reported_async(self, extractor):
        price = named(symbols_for(extractor, "rust"))["price"]
        assert price.is_async
        assert price.signature.startswith("async fn ")

    @pytest.mark.xfail(
        strict=True,
        reason=(
            "B05-H4, the case that costs most: `#[cfg(test)] mod tests { use super::*; }` "
            "is how every Rust crate writes its tests. Measured: the block below yields "
            "zero symbols and zero imports, so a Rust repository's test module is invisible "
            "to both the symbol map and the import graph. Fixed by stage 6c."
        ),
    )
    def test_a_cfg_test_module_contributes_its_items(self, extractor):
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

    @pytest.mark.xfail(
        strict=True,
        reason=(
            "B05-H1: _extract_c_symbols iterates root children only. `#ifndef` wraps the "
            "whole translation unit in one preproc_ifdef, so a guarded header's symbols "
            "are all one level too deep. Measured: pricing.h yields 0 symbols. Nearly "
            "every C header in a real repository is guarded. Fixed by stage 6c."
        ),
    )
    def test_a_guarded_header_yields_its_declarations(self, extractor):
        found = named(symbols_for(extractor, "c_header"))
        assert "Money" in found
        assert "apply_tax_inline" in found

    @pytest.mark.xfail(
        strict=True,
        reason=(
            "B05-H1, import half: the same root-children-only walk in _extract_c_imports. "
            "Measured: pricing.h yields 0 imports, so a guarded header contributes no "
            "edges to the import graph either. Fixed by stage 6c."
        ),
    )
    def test_a_guarded_header_yields_its_includes(self, extractor):
        assert [statement.module for statement in imports_for(extractor, "c_header")] == ["money.h"]


class TestCppNamespace:
    def test_a_declaration_outside_any_namespace_is_extracted(self, extractor):
        # The positive control: the same file's root-level members do arrive,
        # so the namespace test below is about depth, not about C++ support.
        found = named(symbols_for(extractor, "cpp"))
        assert found["Ledger"].signature == "class Ledger"
        assert found["top_level_total"].kind is SymbolKind.FUNCTION

    @pytest.mark.xfail(
        strict=True,
        reason=(
            "B05-H1: a C++ namespace_definition is a root child whose members are one "
            "level deeper, and _extract_c_symbols never descends. Measured: pricing.cpp "
            "yields 2 symbols, and Invoice and billing::apply_tax are not among them. "
            "Fixed by stage 6c."
        ),
    )
    def test_a_declaration_inside_a_namespace_is_extracted(self, extractor):
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

    @pytest.mark.xfail(
        strict=True,
        reason=(
            "B05: the generic walker hardcodes is_public=True for all sixteen of its "
            "languages. Measured: `private void hidden()` comes back is_public=True, and "
            "the value is persisted to the tag cache, so the column is wrong on disk too."
        ),
    )
    def test_a_private_method_is_not_reported_public(self, extractor):
        source = "class C {\n    private void hidden() {}\n    public void shown() {}\n}\n"
        found = named(extractor.extract_from_source(source, "java", "C.java"))
        assert found["hidden"].is_public is False
        assert found["shown"].is_public is True

    @pytest.mark.xfail(
        strict=True,
        reason=(
            "B05: the generic walker's constant branch does not match a Java field "
            "declaration. Measured: `static final double TAX_RATE = 0.2` yields no symbol, "
            "so the fixture's only constant is invisible."
        ),
    )
    def test_a_static_final_field_is_a_constant(self, extractor):
        assert named(symbols_for(extractor, "java"))["TAX_RATE"].kind is SymbolKind.CONSTANT
