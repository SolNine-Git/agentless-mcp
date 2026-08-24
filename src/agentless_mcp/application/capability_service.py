"""Build and render the runtime capability contract shared by both adapters."""

from collections.abc import Sequence
from dataclasses import dataclass, replace
from importlib import metadata
from pathlib import Path
from typing import Any

from agentless_mcp.application import envelope, graph_service, symbol_service
from agentless_mcp.application.map_service import (
    DEFAULT_MAX_FILES,
    GRANULARITY_FUNCTION,
)
from agentless_mcp.application.repo_context import RepoContext
from agentless_mcp.application.symbol_service import DEFAULT_EXPAND_LIMIT
from agentless_mcp.core import (
    cache,
    communities,
    grammars,
    htmlgraph,
    locs,
    mermaid,
    projectconfig,
    resolve,
)
from agentless_mcp.core.extractor import TreeSitterExtractor
from agentless_mcp.core.treewalk import DEFAULT_MAX_ENTRIES, DEFAULT_RENDER_DEPTH
from agentless_mcp.prompts import MESSAGES
from agentless_mcp.util.fslimits import (
    DEFAULT_MAX_DEPTH,
    DEFAULT_MAX_FILE_BYTES,
    DEFAULT_MAX_WALK_FILES,
)

# What a cold language reports while this process's own background warm is
# still running. :func:`grammars.loaded_capabilities` cannot say it: it
# answers one language at a time and the warm is a process-wide fact. Said
# here for the reason the cache half of this report already says it -- advice
# to run ``warmup`` would race the warm already doing exactly that, and a
# tier-1 language listed as unavailable with nothing beside it reads as
# permanent when it is a few seconds old.
WARM_IN_PROGRESS = "not warmed: a background warm is in progress, retry shortly"

# What the roots line says when there is no allowlist at all. ``None`` and an
# empty sequence are different configurations -- see
# :mod:`agentless_mcp.application.repo_context` -- and rendering both as
# "none configured" told a reader that a CLI invocation may touch nothing.
UNRESTRICTED_ROOTS = "unrestricted (CLI mode)"


@dataclass(frozen=True)
class CapabilityReport:
    """Everything a caller needs to explain availability or a bounded answer."""

    version: str
    pack_version: str
    grammar_cache: str
    cache_status: cache.CacheStatus
    cache_hint: str
    languages: tuple[grammars.LanguageCapability, ...]
    extensions: tuple[tuple[str, str], ...]
    config: projectconfig.ProjectConfig
    effective_config: tuple[tuple[str, object], ...]
    # float rather than int: one bound is a modularity resolution.
    caps: tuple[tuple[str, float], ...]
    # ``None`` is "no allowlist at all", which is CLI mode. An empty tuple is
    # a server configured with no roots. The two are different answers.
    configured_roots: tuple[str, ...] | None
    client_roots: tuple[str, ...]

    def as_dict(self) -> dict[str, Any]:
        """Return the complete machine-readable capability contract."""
        return {
            "version": self.version,
            "pack_version": self.pack_version,
            "grammar_cache": self.grammar_cache,
            "cache": self.cache_status.as_dict(),
            "languages": [
                {
                    "name": cap.name,
                    "tier": cap.tier,
                    "abi": cap.abi_version,
                    "warmed": cap.warmed,
                    "probe_ok": cap.probe_ok,
                    "detail": cap.detail,
                }
                for cap in self.languages
            ],
            "extensions": dict(self.extensions),
            "config": self.config.as_dict(),
            "effective_config": dict(self.effective_config),
            "caps": dict(self.caps),
            # The hint is the one actionable line in the report, so the two
            # renderings have to carry it. The CLI builds both up front so
            # `--json` cannot diverge from the text output by accident, and
            # this field is what makes that true of the remediation as well.
            "cache_hint": self.cache_hint,
            "roots": {
                "configured": (
                    None if self.configured_roots is None else list(self.configured_roots)
                ),
                "client": list(self.client_roots),
            },
        }


def build_capability_report(
    ctx: RepoContext,
    extractor: TreeSitterExtractor,
    *,
    configured_roots: Sequence[Path] | None = None,
    client_roots: Sequence[Path] = (),
) -> CapabilityReport:
    """Collect one request's capability facts without adapter-specific rendering.

    ``configured_roots`` is the operator's allowlist, and ``None`` means there
    is none -- the CLI, whose root comes from its own process. A server that
    was started with no roots passes an empty sequence, which is a different
    configuration and reads differently in the report.
    """
    status = _cache_status(ctx, extractor)
    hint = (
        MESSAGES.cache_build_hint.format(repo_root=ctx.root)
        if status.enabled and status.generation is None
        else ""
    )
    return CapabilityReport(
        version=_distribution_version(),
        pack_version=grammars.pack_version(),
        grammar_cache=str(grammars.cache_dir()),
        cache_status=status,
        cache_hint=hint,
        languages=_language_capabilities(),
        extensions=tuple(sorted(TreeSitterExtractor.SUPPORTED_EXTENSIONS.items())),
        config=ctx.config,
        effective_config=_effective_config(ctx.config),
        caps=_caps(),
        configured_roots=(
            None if configured_roots is None else tuple(str(path) for path in configured_roots)
        ),
        client_roots=tuple(str(path) for path in client_roots),
    )


def _cache_status(ctx: RepoContext, extractor: TreeSitterExtractor) -> cache.CacheStatus:
    """Describe this repository's tag cache, measuring it when nobody opened one.

    Both adapters open a source before they call, so ``ctx.symbols`` is
    normally the one the whole request read through and its status is the one
    to report. A library caller need not open anything, and the fallback used
    to be ``OnDemandSource(extractor)`` -- whose status is synthesised, not
    measured: it reports "path None" and tells the caller to run ``index``
    without having looked for a database at all. Opening the real source for
    its status and closing it again is what makes the report about this
    repository rather than about the fallback object.
    """
    if ctx.symbols is not None:
        return ctx.symbols.status()
    source = cache.open_source(ctx.root, extractor, tree_oid=ctx.tree_oid)
    try:
        return source.status()
    finally:
        source.close()


def _language_capabilities() -> tuple[grammars.LanguageCapability, ...]:
    """Report every language, saying which cold ones are being warmed right now."""
    capabilities = tuple(grammars.loaded_capabilities())
    if not grammars.auto_warm_in_progress():
        return capabilities
    return tuple(
        replace(capability, detail=WARM_IN_PROGRESS)
        if not capability.warmed and capability.detail.startswith("not warmed:")
        else capability
        for capability in capabilities
    )


def render_capability_report(report: CapabilityReport) -> str:
    """Render complete capabilities without repeating identical language state."""
    status = report.cache_status
    lines = [
        f"agentless-mcp {report.version}",
        f"pack {report.pack_version}  grammar cache {report.grammar_cache}",
        f"tag cache: {status.receipt}",
        f"  path {status.path}  files {status.files}  tags {status.tags}",
    ]
    if report.cache_hint:
        lines.append(report.cache_hint)
    lines.extend(
        (
            f"roots: {_roots_line(report.configured_roots)}",
            f"client roots: {', '.join(report.client_roots) or 'none advertised'}",
            "languages (name:tier/abi):",
        )
    )
    lines.extend(_language_lines(report.languages))
    lines.append("extensions (language: suffixes):")
    lines.extend(_extension_lines(report.extensions))
    lines.append("effective project config:")
    lines.extend(f"  {name} = {_display(value)}" for name, value in report.effective_config)
    lines.append("caps:")
    lines.extend(f"  {name} = {value}" for name, value in report.caps)
    return "\n".join(lines) + "\n"


def _roots_line(roots: tuple[str, ...] | None) -> str:
    """Render the configured allowlist, keeping "none" apart from "no list"."""
    if roots is None:
        return UNRESTRICTED_ROOTS
    return ", ".join(roots) or "none configured"


def _distribution_version() -> str:
    """Return the installed package version, or a placeholder outside one."""
    try:
        return metadata.version("agentless-mcp")
    except metadata.PackageNotFoundError:
        return "unknown"


def _effective_config(config: projectconfig.ProjectConfig) -> tuple[tuple[str, object], ...]:
    """Resolve repository defaults exactly as the read services consume them."""
    return (
        ("source", str(config.path) if config.path is not None else "none"),
        ("map_budget", config.map_budget if config.map_budget is not None else "auto"),
        ("max_files", projectconfig.resolve(None, config.max_files, DEFAULT_MAX_FILES)),
        (
            "granularity",
            projectconfig.resolve(None, config.granularity, GRANULARITY_FUNCTION),
        ),
        ("docstrings", projectconfig.resolve(None, config.docstrings, False)),
        ("stoplist", tuple(sorted(config.stoplist))),
        (
            "test_cmd",
            "configured (CLI validate only; value hidden)" if config.test_cmd else "none",
        ),
    )


def _caps() -> tuple[tuple[str, float], ...]:
    """Return every bound the services apply, in stable display order.

    "The services apply" rather than the older "every public bound in force",
    which was false: this reported eight of the twenty-three numbers a caller
    can set or hit, so a caller reading it as an inventory was reading a
    sample. The wire schema's own ceilings are deliberately absent -- they
    live in ``adapters.mcp`` and the layer contract forbids this module from
    importing them, so that surface publishes them in its JSON schema
    instead.

    Hand-maintained, and ``tests/unit/test_capability_service.py`` is what
    keeps it honest: it enumerates the bound-shaped names in each module this
    imports from and fails when one is missing here. An inventory nobody
    checks drifts back to a sample.
    """
    return (
        ("max_walk_depth", DEFAULT_MAX_DEPTH),
        ("max_walk_files", DEFAULT_MAX_WALK_FILES),
        ("max_file_bytes", DEFAULT_MAX_FILE_BYTES),
        ("max_output_tokens", envelope.DEFAULT_MAX_TOKENS),
        ("max_config_warnings", envelope.MAX_CONFIG_WARNINGS),
        ("max_map_files", DEFAULT_MAX_FILES),
        ("default_find_limit", symbol_service.DEFAULT_FIND_LIMIT),
        ("default_refs_limit", symbol_service.DEFAULT_REFS_LIMIT),
        ("max_expand_symbols", DEFAULT_EXPAND_LIMIT),
        ("default_explain_limit", graph_service.DEFAULT_EXPLAIN_LIMIT),
        ("default_cycle_limit", graph_service.DEFAULT_CYCLE_LIMIT),
        ("default_community_limit", graph_service.DEFAULT_COMMUNITY_LIMIT),
        ("default_member_limit", graph_service.DEFAULT_MEMBER_LIMIT),
        ("default_community_resolution", communities.DEFAULT_RESOLUTION),
        ("default_context_lines", locs.DEFAULT_CONTEXT_LINES),
        ("default_tree_depth", DEFAULT_RENDER_DEPTH),
        ("default_tree_entries", DEFAULT_MAX_ENTRIES),
        ("default_diagram_nodes", mermaid.DEFAULT_DIAGRAM_NODES),
        ("default_diagram_edges", mermaid.DEFAULT_DIAGRAM_EDGES),
        ("default_html_nodes", htmlgraph.DEFAULT_HTML_NODES),
        ("default_html_edges", htmlgraph.DEFAULT_HTML_EDGES),
        ("max_html_nodes", htmlgraph.MAX_HTML_NODES),
        ("max_html_edges", htmlgraph.MAX_HTML_EDGES),
        ("max_path_visited", resolve.DEFAULT_MAX_VISITED),
    )


def _language_lines(languages: Sequence[grammars.LanguageCapability]) -> list[str]:
    """Group normal language states and spell exceptional failures separately."""
    warmed: list[str] = []
    warming: list[str] = []
    unavailable: list[str] = []
    exceptional: list[str] = []
    for capability in languages:
        entry = f"{capability.name}:{capability.tier}/{capability.abi_version or '-'}"
        if capability.warmed and capability.probe_ok and not capability.detail:
            warmed.append(entry)
        # Before the "not warmed:" arm below, which this detail also matches.
        elif capability.detail == WARM_IN_PROGRESS:
            warming.append(entry)
        elif (
            not capability.warmed
            and not capability.probe_ok
            and capability.detail.startswith("not warmed:")
        ):
            unavailable.append(entry)
        else:
            state = (
                f"warmed={str(capability.warmed).lower()} probe={str(capability.probe_ok).lower()}"
            )
            detail = f" -- {capability.detail}" if capability.detail else ""
            exceptional.append(f"  {state}: {entry}{detail}")

    lines: list[str] = []
    if warmed:
        lines.append(f"  warmed+probe: {', '.join(warmed)}")
    if warming:
        lines.append(f"  warming now: {', '.join(warming)}")
    if unavailable:
        lines.append(f"  unavailable: {', '.join(unavailable)}")
    lines.extend(exceptional)
    return lines


def _extension_lines(extensions: Sequence[tuple[str, str]]) -> list[str]:
    """Group extensions by the language that claims them."""
    grouped: dict[str, list[str]] = {}
    for suffix, language in extensions:
        grouped.setdefault(language, []).append(suffix)
    return [
        f"  {language}: {', '.join(sorted(suffixes))}"
        for language, suffixes in sorted(grouped.items())
    ]


def _display(value: object) -> str:
    """Render compact scalar and list config values deterministically."""
    if isinstance(value, bool):
        return str(value).lower()
    if isinstance(value, tuple):
        return ", ".join(str(item) for item in value) or "none"
    return str(value)
