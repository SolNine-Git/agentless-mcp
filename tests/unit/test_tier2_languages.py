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

from agentless_mcp.core import grammars, skeleton
from agentless_mcp.core.extractor import (
    LANGUAGE_CONFIGS,
    TreeSitterExtractor,
    collect_refs,
    identifier_node_types,
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
    "ruby": "pricing.rb",
    "lua": "pricing.lua",
    "bash": "pricing.sh",
}

# What each language's fixture must yield: the top-level function, the type
# holding the price method, and the qualified name of that method. Bash has no
# type system to speak of and lua's dedicated handler reports a module table,
# so those two carry only what they really have.
EXPECTED = {
    "php": ("apply_tax", "Invoice", "Invoice.price"),
    "kotlin": ("applyTax", "Invoice", "Invoice.price"),
    "swift": ("applyTax", "Invoice", "Invoice.price"),
    "ruby": ("apply_tax", "Invoice", "Invoice.price"),
    "lua": ("apply_tax", "Invoice", None),
    "bash": ("apply_tax", None, None),
}

LANGUAGES = tuple(FIXTURE_FILES)


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


class TestRegistry:
    def test_every_tier2_language_has_a_config_row(self):
        for name in grammars.TIER2_LANGUAGES:
            assert name in LANGUAGE_CONFIGS

    def test_every_tier2_language_has_an_extension(self):
        mapped = set(TreeSitterExtractor.SUPPORTED_EXTENSIONS.values())
        assert set(grammars.TIER2_LANGUAGES) <= mapped

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

    def test_imports_are_extracted(self, language, extractor):
        text, path = source_for(language)
        modules = {
            statement.module
            for statement in extractor.extract_imports_from_source(text, language, path)
        }
        assert modules, f"{language} extracted no imports from {path}"


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
