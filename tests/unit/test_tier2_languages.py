"""The tier-2 language table, against the pack's real grammars.

Every assertion here is a claim about a node-type table that was read off an
actual parse tree rather than off documentation, so the fixtures are checked
in and the tests parse them for real -- a mocked parser would prove that the
table matches itself.

The suite is grammar-dependent by construction, and a grammar the local pack
cache has never downloaded is a cold cache rather than a failure: each test
skips with the language named and the command that warms it. Cold-cache skips
and a red suite must not look the same.
"""

from pathlib import Path

import pytest

from agentless_mcp.core import grammars, graph, refs, skeleton
from agentless_mcp.core.extractor import (
    LANGUAGE_CONFIGS,
    TreeSitterExtractor,
    UnsupportedLanguageError,
    collect_refs,
    identifier_node_types,
    walk_nodes,
)
from agentless_mcp.core.symbols import LANGUAGE_PREFIXES

FIXTURES = Path(__file__).parent.parent / "characterization" / "fixtures" / "tier2"

# One fixture per language, all six spelling the same tiny pricing module, so
# a table that extracts nothing is visible as a missing symbol rather than as
# a differently shaped file.
FIXTURE_FILES = {
    "php": "pricing.php",
    "kotlin": "Pricing.kt",
    "swift": "Pricing.swift",
    "scala": "Pricing.scala",
    "csharp": "Pricing.cs",
    "ruby": "pricing.rb",
    "lua": "pricing.lua",
    "bash": "pricing.sh",
}

# What each language's fixture must yield: the top-level function, the type
# holding the price method, the qualified name of that method, and the one
# module the fixture imports. Bash has no type system to speak of and lua's
# dedicated handler reports a module table, so those two carry only what they
# really have.
#
# The module is spelled in full. Each fixture has exactly one import, so a
# truncated path -- `pricing` where the source wrote `pricing.Money` -- is a
# wrong module rather than a missing one, and nothing but the expected string
# can catch that.
EXPECTED = {
    "php": ("apply_tax", "Invoice", "Invoice.price", "App\\Money\\Currency"),
    "kotlin": ("applyTax", "Invoice", "Invoice.price", "app.money.Currency"),
    "swift": ("applyTax", "Invoice", "Invoice.price", "Foundation"),
    "scala": ("applyTax", "Invoice", "Invoice.price", "pricing.Money"),
    "csharp": ("ApplyTax", "Invoice", "Invoice.Price", "Money.Core"),
    "ruby": ("apply_tax", "Invoice", "Invoice.price", "json"),
    "lua": ("apply_tax", "Invoice", None, "app.money"),
    "bash": ("apply_tax", None, None, "./money.sh"),
}

LANGUAGES = tuple(FIXTURE_FILES)

# One sample per surface that spells every identifier node type its config row
# declares, with the name each one must produce. Quoted keys and quoted values
# are the norm in manifests and CI configs, so they belong in the sample rather
# than only in a bug report.
IDENTIFIER_SAMPLES = {
    # The JSON key carries an escape. The grammar emits `string_content` as a
    # repeat around `escape_sequence`, so a row naming the fragment reported
    # `esc` and `key` -- two names the document never spells.
    "json": ('{"bare": "value", "esc\\tkey": 1}', {"bare", "value", "esc\\tkey"}),
    # TOML's row names key nodes only, so the value is not a name here.
    "toml": ('bare = "value"\n"quoted key" = 1\n', {"bare", "quoted key"}),
    "yaml": (
        "plain: bare\n\"dq key\": \"dq val\"\n'sq key': 'sq val'\n",
        {"plain", "bare", "dq key", "dq val", "sq key", "sq val"},
    ),
    "hcl": (
        'resource "aws_s3_bucket" "logs" {\n  bucket = "prefix-${var.suffix}"\n}\n',
        {"resource", "bucket", "var", "suffix"},
    ),
    "sql": ("CREATE TABLE teams (id INT);\n", {"teams", "id"}),
}

SURFACE_FIXTURES = {
    "json": ("config.json", {"service", "port", "enabled"}),
    "toml": ("config.toml", {"service", "port", "enabled"}),
    "yaml": ("config.yaml", {"service", "port", "enabled"}),
    "hcl": ("main.tf", {"logs", "bucket", "network", "source"}),
    "sql": ("schema.sql", {"teams", "users", "id", "team_id", "active_users"}),
}


def source_for(language: str) -> tuple[str, str]:
    """Return one language's fixture text and repository-relative path."""
    name = FIXTURE_FILES[language]
    return (FIXTURES / name).read_text(encoding="utf-8"), name


@pytest.fixture(params=LANGUAGES)
def language(request):
    """Each tier-2 language in turn, skipped when its grammar is not warmed."""
    name = request.param
    if name not in grammars.warmed_languages():
        pytest.skip(f"grammar for {name} is not in the local pack cache: run agentless-mcp warmup")
    return name


@pytest.fixture(params=tuple(SURFACE_FIXTURES))
def surface_language(request):
    """Each deterministic non-code surface, skipped when not warmed."""
    name = request.param
    if name not in grammars.warmed_languages():
        pytest.skip(f"grammar for {name} is not in the local pack cache: run agentless-mcp warmup")
    return name


class TestRegistry:
    def test_every_tier2_language_has_a_config_row(self):
        for name in grammars.TIER2_LANGUAGES:
            assert name in LANGUAGE_CONFIGS

    def test_every_tier2_language_has_an_extension(self):
        mapped = set(TreeSitterExtractor.SUPPORTED_EXTENSIONS.values())
        assert set(grammars.TIER2_LANGUAGES) <= mapped

    def test_every_supported_extension_has_identifier_node_types(self):
        # `identifier_node_types` raises for a language in neither table
        # rather than handing back a plausible default. The scanner routes by
        # extension, so every language the extension map can produce has to be
        # in one of the tables or a repository scan aborts on that file.
        for language in set(TreeSitterExtractor.SUPPORTED_EXTENSIONS.values()):
            assert identifier_node_types(language)

    def test_a_language_in_no_table_is_refused_rather_than_defaulted(self):
        # The default set is a plausible answer to a different question. An
        # unrecognised name must read as "I do not know this language", not as
        # "this language has one identifier node type".
        with pytest.raises(UnsupportedLanguageError):
            identifier_node_types("pythn")

    def test_every_tier2_language_has_a_stable_id_prefix(self):
        for name in grammars.TIER2_LANGUAGES:
            assert name in LANGUAGE_PREFIXES

    def test_every_tier2_language_has_a_probe_sample(self, language):
        # The probe is what makes a broken grammar a degraded row instead of a
        # wrong answer, so a language without one is not really registered.
        (capability,) = grammars.loaded_capabilities([language])
        assert capability.probe_ok, capability.detail

    def test_a_broken_tier2_grammar_does_not_fail_the_tier1_set(self):
        report = grammars.WarmupReport(
            cache_dir="/tmp/none",
            pack_version="0",
            languages=(
                grammars.LanguageCapability("python", 15, "0", warmed=True, probe_ok=True),
                grammars.LanguageCapability(
                    "kotlin", None, "0", warmed=False, probe_ok=False, detail="fetch failed"
                ),
            ),
        )
        assert [cap.name for cap in report.degraded] == ["kotlin"]
        assert report.degraded_tier1 == ()


# One private and one unmarked declaration per language that spells
# visibility with a keyword. The keyword is what `is_public` must answer
# from: every one of these names would read as public under the
# leading-underscore convention the walker used before stage 6c.
VISIBILITY_SOURCES = {
    "php": "<?php\nclass C { private function hidden() {} public function shown() {} }\n",
    "csharp": "class C { private void hidden() {} public void shown() {} }\n",
    "kotlin": "class C {\n    private fun hidden() {}\n    fun shown() {}\n}\n",
    "scala": "class C {\n    private def hidden(): Unit = {}\n    def shown(): Unit = {}\n}\n",
}


class TestVisibilityKeywords:
    """`is_public` is persisted to the tag cache and filters an overview."""

    @pytest.mark.parametrize("language", sorted(VISIBILITY_SOURCES))
    def test_a_private_declaration_is_not_reported_public(self, language, extractor):
        if language not in grammars.warmed_languages():
            pytest.skip(f"grammar for {language} is not in the local pack cache")
        found = {
            symbol.name: symbol
            for symbol in extractor.extract_from_source(
                VISIBILITY_SOURCES[language], language, f"C.{language}"
            )
        }
        assert found["hidden"].is_public is False
        assert found["shown"].is_public is True

    def test_a_php_constructor_is_public_when_the_keyword_says_so(self, extractor):
        # `__construct` starts with an underscore and is declared public. The
        # convention and the keyword disagree, and the keyword is the answer.
        if "php" not in grammars.warmed_languages():
            pytest.skip("grammar for php is not in the local pack cache")
        source = "<?php\nclass C { public function __construct() {} }\n"
        (symbol,) = [
            s for s in extractor.extract_from_source(source, "php", "C.php") if s.name != "C"
        ]
        assert symbol.is_public is True


class TestSymbolExtraction:
    def test_the_top_level_function_is_extracted(self, language, extractor):
        text, path = source_for(language)
        names = {symbol.name for symbol in extractor.extract_from_source(text, language, path)}
        assert EXPECTED[language][0] in names

    def test_the_declared_type_is_extracted(self, language, extractor):
        expected = EXPECTED[language][1]
        if expected is None:
            pytest.skip(f"{language} declares no type in the fixture")
        text, path = source_for(language)
        names = {symbol.name for symbol in extractor.extract_from_source(text, language, path)}
        assert expected in names

    def test_a_method_carries_its_owning_type(self, language, extractor):
        expected = EXPECTED[language][2]
        if expected is None:
            pytest.skip(f"{language} has no class-owned method in the fixture")
        text, path = source_for(language)
        qualified = {
            f"{symbol.parent_class}.{symbol.name}" if symbol.parent_class else symbol.name
            for symbol in extractor.extract_from_source(text, language, path)
        }
        assert expected in qualified

    def test_a_signature_never_carries_the_body(self, language, extractor):
        text, path = source_for(language)
        for symbol in extractor.extract_from_source(text, language, path):
            assert "return" not in symbol.signature, symbol.signature

    def test_the_imported_module_is_extracted_in_full(self, language, extractor):
        text, path = source_for(language)
        modules = {
            statement.module
            for statement in extractor.extract_imports_from_source(text, language, path)
        }
        assert modules == {EXPECTED[language][3]}, f"{language} from {path}"


class TestDeterministicNonCodeSurfaces:
    def test_config_definitions_feed_the_same_cross_file_graph(self, tmp_path, extractor):
        if "json" not in grammars.warmed_languages():
            pytest.skip("grammar for json is not in the local pack cache")
        (tmp_path / "config.json").write_text('{"service": {"port": 8080}}\n', encoding="utf-8")
        (tmp_path / "app.py").write_text("def configured():\n    return port\n", encoding="utf-8")

        scan = refs.scan_repo(tmp_path, extractor)
        built = graph.build_graph(scan, refs.build_ref_index(scan))

        assert ("app.py", "config.json") in built.edges

    def test_expected_nodes_feed_the_symbol_graph(self, surface_language, extractor):
        filename, expected = SURFACE_FIXTURES[surface_language]
        text = (FIXTURES / filename).read_text(encoding="utf-8")

        symbols = extractor.extract_from_source(text, surface_language, filename)

        assert expected <= {symbol.name for symbol in symbols}
        assert all(symbol.language == surface_language for symbol in symbols)

    def test_references_come_from_parsed_name_nodes(self, surface_language):
        filename, expected = SURFACE_FIXTURES[surface_language]
        text = (FIXTURES / filename).read_text(encoding="utf-8")

        names = {ref.name for ref in collect_refs(text, surface_language, filename)}

        assert expected <= names

    def test_every_declared_identifier_type_reaches_the_reference_pass(self, surface_language):
        """A declared node type the collector can never emit describes nothing.

        The config row and the collector's leaf rule are two halves of one
        contract kept in two places. When they disagree the surface reports
        zero references for a whole construct instead of an error, which is
        the most expensive answer this tool gives: a quoted TOML key and
        every quoted YAML scalar were invisible under the old rule.
        """
        text, expected = IDENTIFIER_SAMPLES[surface_language]
        declared = identifier_node_types(surface_language)

        tree = grammars.get_parser(surface_language).parse(text.encode("utf-8"))
        spelled = {node.type for node in walk_nodes(tree.root_node)} & declared
        names = {ref.name for ref in collect_refs(text, surface_language, "sample")}

        assert spelled == declared, f"{surface_language} sample spells only {sorted(spelled)}"
        assert expected <= names

    def test_nested_config_keys_carry_their_owner(self, surface_language, extractor):
        if surface_language not in {"json", "toml", "yaml"}:
            pytest.skip("only structured config files have the service.port shape")
        filename, _ = SURFACE_FIXTURES[surface_language]
        text = (FIXTURES / filename).read_text(encoding="utf-8")

        symbols = extractor.extract_from_source(text, surface_language, filename)
        port = next(symbol for symbol in symbols if symbol.name == "port")

        assert port.parent_class == "service"

    def test_sql_columns_link_to_their_table(self, surface_language, extractor):
        if surface_language != "sql":
            pytest.skip("SQL is the schema surface")
        filename, _ = SURFACE_FIXTURES[surface_language]
        text = (FIXTURES / filename).read_text(encoding="utf-8")

        symbols = extractor.extract_from_source(text, surface_language, filename)

        assert any(
            symbol.name == "team_id" and symbol.parent_class == "users" for symbol in symbols
        )


class TestSkeleton:
    def test_the_skeleton_keeps_declarations_and_drops_bodies(self, language):
        text, _ = source_for(language)
        rendered = skeleton.skeletonize(text, language)

        assert EXPECTED[language][0] in rendered
        assert skeleton.SENTINEL in rendered
        assert "1 + rate" not in rendered

    def test_the_skeleton_is_shorter_than_the_source(self, language):
        text, _ = source_for(language)
        assert skeleton.compression_ratio(text, skeleton.skeletonize(text, language)) > 1.0


class TestReferences:
    def test_the_identifier_pass_finds_the_call_site(self, language):
        text, path = source_for(language)
        names = {ref.name for ref in collect_refs(text, language, path)}
        assert EXPECTED[language][0] in names

    def test_every_reference_carries_a_line_inside_the_file(self, language):
        text, path = source_for(language)
        line_count = len(text.split("\n"))
        for ref in collect_refs(text, language, path):
            assert 1 <= ref.line <= line_count

    def test_the_identifier_node_types_come_from_the_config_row(self, language):
        assert identifier_node_types(language) == frozenset(
            LANGUAGE_CONFIGS[language].identifier_node_types
        )


# Deep enough that a walker recursing once per child exceeds the default
# recursion limit: lua's chained calls nest two grammar nodes per link and
# bash's command substitutions nest three, so 600 links is well past it.
_DEEP_CHAIN = 600

DEEP_LUA = (
    'local mod = require("mod")\n'
    "local value = client" + "".join(f":step{index}()" for index in range(_DEEP_CHAIN)) + "\n"
)

DEEP_BASH = "source ./lib.sh\necho " + "$(" * _DEEP_CHAIN + "true" + ")" * _DEEP_CHAIN + "\n"


class TestStackSafety:
    """The two dedicated import walks are iterative, like every other walk here.

    Lua/Ruby `require` and bash `source` each walk the whole tree, so a deeply
    nested expression anywhere in the file reaches them. A `RecursionError`
    escaping either one aborts the index for the entire repository.
    """

    def test_a_deep_lua_call_chain_still_finds_the_require(self, extractor):
        if "lua" not in grammars.warmed_languages():
            pytest.skip("grammar for lua is not in the local pack cache: run agentless-mcp warmup")
        imports = extractor.extract_imports_from_source(DEEP_LUA, "lua", "deep.lua")
        assert [statement.module for statement in imports] == ["mod"]

    def test_a_deep_bash_substitution_still_finds_the_source(self, extractor):
        if "bash" not in grammars.warmed_languages():
            pytest.skip("grammar for bash is not in the local pack cache: run agentless-mcp warmup")
        imports = extractor.extract_imports_from_source(DEEP_BASH, "bash", "deep.sh")
        assert [statement.module for statement in imports] == ["./lib.sh"]
