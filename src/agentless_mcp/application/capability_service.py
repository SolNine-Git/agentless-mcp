"""Build and render the runtime capability contract shared by both adapters."""

from collections.abc import Sequence
from dataclasses import dataclass
from importlib import metadata
from pathlib import Path
from typing import Any

from agentless_mcp.application import envelope
from agentless_mcp.application.map_service import (
    DEFAULT_MAX_FILES,
    GRANULARITY_FUNCTION,
)
from agentless_mcp.application.repo_context import RepoContext
from agentless_mcp.application.symbol_service import DEFAULT_EXPAND_LIMIT
from agentless_mcp.core import cache, grammars, projectconfig
from agentless_mcp.core.extractor import TreeSitterExtractor
from agentless_mcp.core.treewalk import DEFAULT_MAX_ENTRIES, DEFAULT_RENDER_DEPTH
from agentless_mcp.prompts import MESSAGES
from agentless_mcp.util.fslimits import DEFAULT_MAX_DEPTH, DEFAULT_MAX_FILE_BYTES
from agentless_mcp.util.fslimits import DEFAULT_MAX_FILES as WALK_MAX_FILES


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
    caps: tuple[tuple[str, int], ...]
    configured_roots: tuple[str, ...]
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
            "roots": {
                "configured": list(self.configured_roots),
                "client": list(self.client_roots),
            },
        }


def build_capability_report(
    ctx: RepoContext,
    extractor: TreeSitterExtractor,
    *,
    configured_roots: Sequence[Path] = (),
    client_roots: Sequence[Path] = (),
) -> CapabilityReport:
    """Collect one request's capability facts without adapter-specific rendering."""
    source = ctx.symbols if ctx.symbols is not None else cache.OnDemandSource(extractor)
    status = source.status()
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
        languages=tuple(grammars.loaded_capabilities()),
        extensions=tuple(sorted(TreeSitterExtractor.SUPPORTED_EXTENSIONS.items())),
        config=ctx.config,
        effective_config=_effective_config(ctx.config),
        caps=_caps(),
        configured_roots=tuple(str(path) for path in configured_roots),
        client_roots=tuple(str(path) for path in client_roots),
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
            f"roots: {', '.join(report.configured_roots) or 'none configured'}",
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


def _caps() -> tuple[tuple[str, int], ...]:
    """Return every public bound in force in stable display order."""
    return (
        ("max_walk_depth", DEFAULT_MAX_DEPTH),
        ("max_walk_files", WALK_MAX_FILES),
        ("max_file_bytes", DEFAULT_MAX_FILE_BYTES),
        ("max_output_tokens", envelope.DEFAULT_MAX_TOKENS),
        ("max_map_files", DEFAULT_MAX_FILES),
        ("max_expand_symbols", DEFAULT_EXPAND_LIMIT),
        ("default_tree_depth", DEFAULT_RENDER_DEPTH),
        ("default_tree_entries", DEFAULT_MAX_ENTRIES),
    )


def _language_lines(languages: Sequence[grammars.LanguageCapability]) -> list[str]:
    """Group normal language states and spell exceptional failures separately."""
    warmed: list[str] = []
    unavailable: list[str] = []
    exceptional: list[str] = []
    for capability in languages:
        entry = f"{capability.name}:{capability.tier}/{capability.abi_version or '-'}"
        if capability.warmed and capability.probe_ok and not capability.detail:
            warmed.append(entry)
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
