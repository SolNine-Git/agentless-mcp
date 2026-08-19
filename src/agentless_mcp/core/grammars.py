"""Grammar loading, version stamping and per-language degradation.

This is the only module allowed to touch ``tree_sitter_language_pack``, and
the only module in the package that can reach the network. Everything else
asks here for a ``Language`` or a ``Parser``.

The pack downloads grammars on first use. That is the right behaviour at
install time and the wrong behaviour in the middle of a tool call, so the two
paths are split:

* :func:`warmup` is the only function that fetches. It reports per language
  instead of raising, so one unavailable grammar degrades one language.
* :func:`get_language` never fetches. A grammar that is not already on disk
  produces :class:`LanguageUnavailable` naming the remediation.

Verified against tree-sitter-language-pack 1.14.3 (2026-08-18):
``downloaded_languages()`` reflects the shared libraries present in the
cache directory, so it is the warmed-state probe -- no marker file of our
own is needed. The pack does NOT read ``TREE_SITTER_LANGUAGE_PACK_CACHE_DIR``
(only ``XDG_CACHE_HOME`` and its own ``PackConfig``), so this module reads
that variable itself and applies it through ``configure()``.
"""

import os
from collections.abc import Sequence
from dataclasses import dataclass
from functools import cache, lru_cache
from importlib import metadata

import tree_sitter_language_pack as pack
from tree_sitter import Language, Parser
from tree_sitter_language_pack import PackConfig

from agentless_mcp.util.errors import AtlasError, LanguageUnavailable

ENV_NO_DOWNLOAD = "AGENTLESS_MCP_NO_DOWNLOAD"
ENV_CACHE_DIR = "TREE_SITTER_LANGUAGE_PACK_CACHE_DIR"

# Tier-1 languages: the twelve ported from the mcp-local extractor, each with
# a hand-checked node-type table and characterization coverage.
TIER1_LANGUAGES: tuple[str, ...] = (
    "bash",
    "c",
    "cpp",
    "go",
    "java",
    "javascript",
    "lua",
    "python",
    "ruby",
    "rust",
    "tsx",
    "typescript",
)

# Tier 2 (2026-08-18, Phase 4): the long tail, wired from the pack's own parse
# trees. Same extraction path and the same probe gate as tier 1; the tier says
# how much evidence stands behind the node-type table, and it is what lets a
# broken tier-2 grammar degrade without failing a warmup that tier 1 passed.
TIER2_LANGUAGES: tuple[str, ...] = (
    "kotlin",
    "php",
    "swift",
)

ALL_LANGUAGES: tuple[str, ...] = tuple(sorted((*TIER1_LANGUAGES, *TIER2_LANGUAGES)))


def is_tier1(language: str) -> bool:
    """True when ``language`` is one this package promises full support for."""
    return language in TIER1_LANGUAGES


# One line of unambiguous source per language. A grammar that cannot parse
# this without an ERROR node is broken for our purposes, whatever the loader
# reported.
_PROBE_SAMPLES: dict[str, str] = {
    "bash": "x=1\n",
    "c": "int main(void) { return 0; }\n",
    "cpp": "int main(void) { return 0; }\n",
    "go": "package main\n",
    "java": "class A {}\n",
    "javascript": "const x = 1;\n",
    "lua": "local x = 1\n",
    "python": "x = 1\n",
    "ruby": "x = 1\n",
    "rust": "fn main() {}\n",
    "tsx": "const x = 1;\n",
    "typescript": "const x: number = 1;\n",
    "kotlin": "val x = 1\n",
    "php": "<?php\n$x = 1;\n",
    "swift": "let x = 1\n",
}


@dataclass(frozen=True)
class LanguageCapability:
    """What is actually available for one language, right now."""

    name: str
    abi_version: int | None
    pack_version: str
    warmed: bool
    probe_ok: bool
    detail: str = ""

    @property
    def tier(self) -> int:
        """Which support tier this language is in."""
        return 1 if is_tier1(self.name) else 2


@dataclass(frozen=True)
class WarmupReport:
    """The outcome of a warmup run, one row per requested language."""

    cache_dir: str
    pack_version: str
    languages: tuple[LanguageCapability, ...]

    @property
    def degraded(self) -> tuple[LanguageCapability, ...]:
        """Languages that did not end up warmed and probe-clean."""
        return tuple(c for c in self.languages if not (c.warmed and c.probe_ok))

    @property
    def degraded_tier1(self) -> tuple[LanguageCapability, ...]:
        """The degraded languages this package promises full support for.

        The distinction the tier exists for: a tier-2 grammar that will not
        load costs that one language, and must not be able to fail a warmup
        that every tier-1 language passed.
        """
        return tuple(c for c in self.degraded if c.tier == 1)

    @property
    def ok(self) -> bool:
        """True when every requested language warmed and probed clean."""
        return not self.degraded


def pack_version() -> str:
    """Return the installed language-pack version."""
    return metadata.version("tree-sitter-language-pack")


def cache_dir() -> str:
    """Return the directory the pack loads grammars from."""
    return _configured_cache_dir()


@lru_cache(maxsize=1)
def _configured_cache_dir() -> str:
    """Apply the cache-dir override once, then report the effective path."""
    override = os.environ.get(ENV_CACHE_DIR)
    if override:
        pack.configure(PackConfig(cache_dir=override))
    return pack.cache_dir()


def no_download_requested() -> bool:
    """True when the environment forbids fetching grammars."""
    return os.environ.get(ENV_NO_DOWNLOAD, "") not in ("", "0", "false", "False")


def warmed_languages() -> frozenset[str]:
    """Return the languages loadable from the local cache without a fetch."""
    _configured_cache_dir()
    return frozenset(pack.downloaded_languages())


def get_language(name: str) -> Language:
    """Return the grammar for ``name`` without ever fetching it.

    Raises :class:`LanguageUnavailable` when the grammar is unknown to the
    pack or is not warmed, so a missing grammar is a message the caller can
    act on rather than a surprise download inside a tool call.
    """
    _configured_cache_dir()

    if not pack.has_language(name):
        message = f"unknown language '{name}': not offered by tree-sitter-language-pack"
        raise LanguageUnavailable(message)

    if name not in warmed_languages():
        message = f"language '{name}' not warmed: run agentless-mcp warmup"
        raise LanguageUnavailable(message)

    try:
        return pack.get_language(name)
    except (pack.Error, RuntimeError) as exc:
        message = f"language '{name}' failed to load from {cache_dir()}: {exc}"
        raise LanguageUnavailable(message) from exc


@cache
def get_parser(name: str) -> Parser:
    """Return a memoized parser for ``name``. Never fetches."""
    return Parser(get_language(name))


def warmup(
    languages: Sequence[str] | None = None,
    *,
    no_download: bool = False,
) -> WarmupReport:
    """Fetch, load and probe-parse each language; report per language.

    This is the only function that downloads. A language that fails to fetch,
    load or probe is reported as degraded; the run continues. The single case
    that raises is a refusal: a fetch is required but downloads are forbidden,
    which is a configuration decision the caller must see, not a degraded row
    they might page past.
    """
    names = tuple(languages) if languages is not None else ALL_LANGUAGES
    blocked = no_download or no_download_requested()
    directory = _configured_cache_dir()
    version = pack_version()

    capabilities = tuple(_warm_one(name, version, blocked=blocked) for name in names)
    return WarmupReport(cache_dir=directory, pack_version=version, languages=capabilities)


def loaded_capabilities(languages: Sequence[str] | None = None) -> list[LanguageCapability]:
    """Report what is available now. Never fetches, never raises."""
    names = tuple(languages) if languages is not None else ALL_LANGUAGES
    version = pack_version()
    warmed = warmed_languages()

    capabilities: list[LanguageCapability] = []
    for name in names:
        if name not in warmed:
            capabilities.append(
                LanguageCapability(
                    name=name,
                    abi_version=None,
                    pack_version=version,
                    warmed=False,
                    probe_ok=False,
                    detail=f"not warmed: run agentless-mcp warmup {name}",
                )
            )
            continue
        capabilities.append(_load_and_probe(name, version))
    return capabilities


def _warm_one(name: str, version: str, *, blocked: bool) -> LanguageCapability:
    """Fetch ``name`` if needed, then load and probe it."""
    if name not in warmed_languages():
        if blocked:
            message = (
                f"refusing to download grammar '{name}': {ENV_NO_DOWNLOAD} is set. "
                f"Warm it on a networked machine and copy {cache_dir()} across, "
                f"or clear the flag and run warmup again."
            )
            raise AtlasError(message)
        try:
            pack.prefetch([name])
        except (pack.Error, RuntimeError) as exc:
            return LanguageCapability(
                name=name,
                abi_version=None,
                pack_version=version,
                warmed=False,
                probe_ok=False,
                detail=f"fetch failed: {exc}",
            )

    return _load_and_probe(name, version)


def _load_and_probe(name: str, version: str) -> LanguageCapability:
    """Load a warmed grammar and parse its probe sample."""
    try:
        language = get_language(name)
    except LanguageUnavailable as exc:
        return LanguageCapability(
            name=name,
            abi_version=None,
            pack_version=version,
            warmed=False,
            probe_ok=False,
            detail=str(exc),
        )

    probe_ok, detail = _probe(name, language)
    return LanguageCapability(
        name=name,
        abi_version=language.abi_version,
        pack_version=version,
        warmed=True,
        probe_ok=probe_ok,
        detail=detail,
    )


def _probe(name: str, language: Language) -> tuple[bool, str]:
    """Parse the language's one-line sample; report why it failed if it did."""
    sample = _PROBE_SAMPLES.get(name)
    if sample is None:
        return False, "no probe sample for this language"

    try:
        tree = Parser(language).parse(sample.encode("utf-8"))
    except (ValueError, RuntimeError) as exc:
        return False, f"probe parse raised: {exc}"

    root = tree.root_node
    if root.has_error:
        return False, "probe parse produced an ERROR node"
    if root.child_count == 0:
        return False, "probe parse produced an empty tree"
    return True, ""
