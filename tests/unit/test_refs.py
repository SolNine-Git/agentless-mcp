"""The identifier-reference pass, the repository scan, and fan-in attribution."""

from agentless_mcp.core.extractor import collect_refs, identifier_node_types
from agentless_mcp.core.refs import (
    build_ref_index,
    enclosing_symbol,
    references_to,
    scan_repo,
)
from agentless_mcp.core.symbols import qualname

LIBRARY = '''\
"""Pricing."""

BASE = 10


def quote(sku):
    return BASE


class PriceBook:
    def cost_of(self, sku):
        return quote(sku)

    def unused(self):
        return None
'''

CALLER = """\
from library import quote


TOTAL = 0


def run_billing(items):
    return sum(quote(item) for item in items)


class Ledger:
    def post(self, item):
        return quote(item)
"""


def build(tmp_path):
    """Write the two-file fixture used by most of this module."""
    (tmp_path / "library.py").write_text(LIBRARY, encoding="utf-8")
    (tmp_path / "caller.py").write_text(CALLER, encoding="utf-8")
    return tmp_path


class TestIdentifierTable:
    def test_ecmascript_carries_property_identifiers(self):
        for language in ("javascript", "typescript", "tsx"):
            assert "property_identifier" in identifier_node_types(language)

    def test_go_carries_field_and_package_identifiers(self):
        assert {"field_identifier", "package_identifier"} <= identifier_node_types("go")

    def test_a_dedicated_handler_language_still_has_a_table_entry(self):
        assert "identifier" in identifier_node_types("python")
        assert "field_identifier" in identifier_node_types("rust")


class TestCollectRefs:
    def test_every_occurrence_is_kept_including_the_declaration(self):
        refs = collect_refs("def quote(sku):\n    return quote(sku)\n", "python", "a.py")
        quotes = [ref for ref in refs if ref.name == "quote"]
        assert [ref.line for ref in quotes] == [1, 2]

    def test_typescript_sees_names_inside_exported_declarations(self):
        source = "export class Widget {\n  render() {\n    return helper();\n  }\n}\n"
        names = {ref.name for ref in collect_refs(source, "typescript", "w.ts")}
        assert {"Widget", "render", "helper"} <= names

    def test_a_parameter_s_occurrences_are_marked_locally_bound(self):
        refs = collect_refs("def f(quote):\n    return quote\n", "python", "a.py")
        bound = [ref for ref in refs if ref.name == "quote"]
        assert [ref.line for ref in bound] == [1, 2]
        assert all(ref.locally_bound for ref in bound)

    def test_annotation_and_default_names_are_not_locally_bound(self):
        source = "def f(quote: Book = LEDGER):\n    return quote\n"
        flags = {
            (ref.name, ref.line): ref.locally_bound
            for ref in collect_refs(source, "python", "a.py")
        }
        assert flags[("quote", 1)]
        assert flags[("quote", 2)]
        assert not flags[("Book", 1)]
        assert not flags[("LEDGER", 1)]

    def test_splat_and_lambda_parameters_bind_too(self):
        source = "def f(*args, **kwargs):\n    g = lambda item: item + args + kwargs\n"
        refs = collect_refs(source, "python", "a.py")
        assert all(ref.locally_bound for ref in refs if ref.name in {"args", "kwargs", "item"})

    def test_a_use_outside_the_binding_function_is_not_marked(self):
        source = "def f(quote):\n    return quote\n\n\nTOTAL = quote\n"
        refs = collect_refs(source, "python", "a.py")
        outside = next(ref for ref in refs if ref.name == "quote" and ref.line == 5)
        assert not outside.locally_bound


class TestScan:
    def test_the_scan_carries_symbols_imports_and_refs_per_file(self, tmp_path, extractor):
        scan = scan_repo(build(tmp_path), extractor)
        facts = scan.by_path()["caller.py"]

        assert {symbol.name for symbol in facts.symbols} >= {"run_billing", "Ledger", "post"}
        assert [statement.module for statement in facts.imports] == ["library"]
        assert facts.refs

    def test_unsupported_file_types_are_skipped_without_a_complaint(self, tmp_path, extractor):
        (tmp_path / "README.md").write_text("# hello\n", encoding="utf-8")
        scan = scan_repo(tmp_path, extractor)
        assert scan.files == ()
        assert scan.skipped == ()

    def test_an_oversized_file_is_reported_not_dropped(self, tmp_path, extractor):
        (tmp_path / "huge.py").write_text("x = 1\n" * 5000, encoding="utf-8")
        scan = scan_repo(tmp_path, extractor, max_file_bytes=100)

        assert scan.files == ()
        assert scan.skipped[0].path == "huge.py"
        assert "exceeds the per-file cap" in scan.skipped[0].reason


class TestFanIn:
    def test_references_exclude_the_symbols_own_body_and_declaration(self, tmp_path, extractor):
        scan = scan_repo(build(tmp_path), extractor)
        index = build_ref_index(scan)
        definition = next(
            entry for entry in index.definitions["quote"] if entry.path == "library.py"
        )

        sites = references_to(index, definition)
        assert all(not (site.path == "library.py" and 6 <= site.line <= 7) for site in sites)
        assert {site.path for site in sites} == {"library.py", "caller.py"}

    def test_fan_in_still_lists_sites_where_the_name_is_a_parameter(self, tmp_path, extractor):
        (tmp_path / "library.py").write_text(LIBRARY, encoding="utf-8")
        (tmp_path / "borrower.py").write_text(
            "def take(quote):\n    return quote\n", encoding="utf-8"
        )
        index = build_ref_index(scan_repo(tmp_path, extractor))
        definition = next(
            entry for entry in index.definitions["quote"] if entry.path == "library.py"
        )
        assert any(site.path == "borrower.py" for site in references_to(index, definition))

    def test_an_unreferenced_symbol_has_no_sites(self, tmp_path, extractor):
        scan = scan_repo(build(tmp_path), extractor)
        index = build_ref_index(scan)
        definition = index.definitions["unused"][0]
        assert references_to(index, definition) == ()

    def test_each_site_is_attributed_to_its_innermost_enclosing_symbol(self, tmp_path, extractor):
        scan = scan_repo(build(tmp_path), extractor)
        index = build_ref_index(scan)
        by_path = scan.by_path()
        definition = next(
            entry for entry in index.definitions["quote"] if entry.path == "library.py"
        )

        attributed = {
            (site.path, qualname(enclosing_symbol(by_path[site.path], site.line)))
            for site in references_to(index, definition)
            if enclosing_symbol(by_path[site.path], site.line) is not None
        }
        assert ("caller.py", "run_billing") in attributed
        assert ("caller.py", "Ledger.post") in attributed
        assert ("library.py", "PriceBook.cost_of") in attributed

    def test_a_module_level_reference_has_no_enclosing_symbol(self, tmp_path, extractor):
        scan = scan_repo(build(tmp_path), extractor)
        facts = scan.by_path()["caller.py"]
        assert enclosing_symbol(facts, 1) is None

    def test_a_method_wins_over_the_class_that_contains_it(self, tmp_path, extractor):
        scan = scan_repo(build(tmp_path), extractor)
        facts = scan.by_path()["caller.py"]
        symbol = enclosing_symbol(facts, CALLER.split("\n").index("        return quote(item)") + 1)
        assert symbol is not None
        assert qualname(symbol) == "Ledger.post"


class TestIndex:
    def test_files_referencing_counts_distinct_files_not_occurrences(self, tmp_path, extractor):
        index = build_ref_index(scan_repo(build(tmp_path), extractor))
        assert index.files_referencing["quote"] == 2

    def test_defining_paths_are_sorted_and_deduplicated(self, tmp_path, extractor):
        index = build_ref_index(scan_repo(build(tmp_path), extractor))
        assert index.defining_paths("quote") == ("library.py",)
