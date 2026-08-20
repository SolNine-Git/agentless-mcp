"""The identifier-reference pass, the repository scan, and fan-in attribution."""

from agentless_mcp.core.extractor import IdentifierRole, collect_refs, identifier_node_types
from agentless_mcp.core.refs import (
    build_ref_index,
    enclosing_symbol,
    line_owners,
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

    def test_a_parameter_s_occurrences_are_marked_local(self):
        refs = collect_refs("def f(quote):\n    return quote\n", "python", "a.py")
        bound = [ref for ref in refs if ref.name == "quote"]
        assert [ref.line for ref in bound] == [1, 2]
        assert [ref.role for ref in bound] == [IdentifierRole.BINDING, IdentifierRole.LOCAL]

    def test_annotation_and_default_names_remain_references(self):
        source = "def f(quote: Book = LEDGER):\n    return quote\n"
        roles = {(ref.name, ref.line): ref.role for ref in collect_refs(source, "python", "a.py")}
        assert roles[("quote", 1)] is IdentifierRole.BINDING
        assert roles[("quote", 2)] is IdentifierRole.LOCAL
        assert roles[("Book", 1)] is IdentifierRole.REFERENCE
        assert roles[("LEDGER", 1)] is IdentifierRole.REFERENCE

    def test_splat_and_lambda_parameters_bind_too(self):
        source = "def f(*args, **kwargs):\n    g = lambda item: item + args + kwargs\n"
        refs = collect_refs(source, "python", "a.py")
        assert all(not ref.is_reference for ref in refs if ref.name in {"args", "kwargs", "item"})

    def test_a_use_outside_the_binding_function_is_not_marked(self):
        source = "def f(quote):\n    return quote\n\n\nTOTAL = quote\n"
        refs = collect_refs(source, "python", "a.py")
        outside = next(ref for ref in refs if ref.name == "quote" and ref.line == 5)
        assert outside.role is IdentifierRole.REFERENCE

    def test_nested_declarations_bind_in_the_enclosing_function(self):
        source = (
            "def outer():\n"
            "    def inner():\n"
            "        return 1\n"
            "    class Local:\n"
            "        pass\n"
            "    return inner(), Local()\n"
        )
        refs = collect_refs(source, "python", "a.py")
        roles = {(ref.name, ref.line): ref.role for ref in refs}

        assert roles[("inner", 2)] is IdentifierRole.DECLARATION
        assert roles[("inner", 6)] is IdentifierRole.LOCAL
        assert roles[("Local", 4)] is IdentifierRole.DECLARATION
        assert roles[("Local", 6)] is IdentifierRole.LOCAL

    def test_assignment_and_loop_bindings_do_not_become_references(self):
        source = (
            "def f(rows):\n"
            "    counter = 0\n"
            "    for item in rows:\n"
            "        counter += item\n"
            "    return counter\n"
        )
        refs = collect_refs(source, "python", "a.py")

        assert all(not ref.is_reference for ref in refs if ref.name in {"counter", "item"})

    def test_keyword_and_attribute_names_have_non_reference_roles(self):
        refs = collect_refs(
            "def f(stream, value):\n    return call(graphs=value, stderr=stream.write)\n",
            "python",
            "a.py",
        )
        roles = {(ref.name, ref.line): ref.role for ref in refs}

        assert roles[("graphs", 2)] is IdentifierRole.KEYWORD
        assert roles[("stderr", 2)] is IdentifierRole.KEYWORD
        assert roles[("write", 2)] is IdentifierRole.ATTRIBUTE
        assert roles[("call", 2)] is IdentifierRole.REFERENCE

    def test_only_a_direct_imported_module_member_is_resolvable(self):
        refs = collect_refs(
            'import sys\nsys.stderr.write("x")\n',
            "python",
            "a.py",
        )
        by_name = {ref.name: ref for ref in refs if ref.line == 2}

        assert by_name["sys"].role is IdentifierRole.MODULE_QUALIFIER
        assert by_name["stderr"].role is IdentifierRole.MODULE_ATTRIBUTE
        assert by_name["stderr"].qualifier == "sys"
        assert by_name["write"].role is IdentifierRole.ATTRIBUTE
        assert not by_name["write"].is_resolvable

    def test_an_import_alias_preserves_the_source_qualifier(self):
        refs = collect_refs(
            "import core as c\nc.only_once()\n",
            "python",
            "a.py",
        )
        by_name = {ref.name: ref for ref in refs if ref.line == 2}

        assert by_name["c"].role is IdentifierRole.MODULE_QUALIFIER
        assert by_name["only_once"].role is IdentifierRole.MODULE_ATTRIBUTE
        assert by_name["only_once"].qualifier == "core"

    def test_a_local_binding_shadows_an_imported_module(self):
        refs = collect_refs(
            "import core\n\n\ndef use(core):\n    return core.only_once()\n",
            "python",
            "a.py",
        )
        by_name = {ref.name: ref for ref in refs if ref.line == 5}

        assert by_name["core"].role is IdentifierRole.LOCAL
        assert by_name["only_once"].role is IdentifierRole.ATTRIBUTE

    def test_import_syntax_is_not_a_reference_but_imported_use_is(self):
        refs = collect_refs(
            "from library import helper as alias\n\n\ndef f():\n    return alias()\n",
            "python",
            "a.py",
        )
        aliases = [ref for ref in refs if ref.name == "alias"]

        assert [ref.role for ref in aliases] == [IdentifierRole.IMPORT, IdentifierRole.REFERENCE]

    def test_with_exception_and_comprehension_targets_are_local(self):
        source = (
            "def f(source):\n"
            "    with open('x') as handle:\n"
            "        values = [entry for entry in source]\n"
            "    try:\n"
            "        return handle, values\n"
            "    except Error as exc:\n"
            "        return exc\n"
        )
        refs = collect_refs(source, "python", "a.py")

        assert all(
            not ref.is_reference for ref in refs if ref.name in {"handle", "values", "entry", "exc"}
        )

    def test_global_declaration_keeps_uses_resolvable(self):
        source = "def f():\n    global counter\n    return counter\n"
        counters = [ref for ref in collect_refs(source, "python", "a.py") if ref.name == "counter"]

        assert [ref.role for ref in counters] == [IdentifierRole.BINDING, IdentifierRole.REFERENCE]


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

    def test_the_bulk_owner_map_agrees_with_the_point_query(self, tmp_path, extractor):
        """One innermost-symbol rule, asked two ways, on every line of a file.

        The resolver builds the whole map once per file and the fan-in views
        ask line by line; the two must never drift apart about which symbol
        owns a line, tie-breaks included.
        """
        scan = scan_repo(build(tmp_path), extractor)
        facts = scan.by_path()["library.py"]
        owners = line_owners(facts)

        assert {line: owners.get(line) for line in range(1, facts.line_count + 1)} == {
            line: enclosing_symbol(facts, line) for line in range(1, facts.line_count + 1)
        }
        assert owners

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
