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

Verified 2026-08-22, issue #19: the wheel ships no grammar binaries. The
first ``prefetch`` against a cold cache downloads the whole platform bundle
(``bundles/<platform>-<sha256>.tar.zst`` beside the libs directory, digest
in the name, sha-256 verified by the pack) plus ``manifest.json``; every
later warm -- any language, networking disabled -- is a local extraction
from that cached bundle, ~3s for all 22. The background warm below
therefore fetches at most once per cache directory, and on a machine whose
cache already holds the bundle it never fetches at all.
"""

import logging
import os
import threading
import time
from collections.abc import Sequence
from dataclasses import dataclass
from functools import lru_cache
from importlib import metadata
from pathlib import Path

import tree_sitter_language_pack as pack
from tree_sitter import Language, Parser
from tree_sitter_language_pack import PackConfig

from agentless_mcp.util.errors import AgentlessError, LanguageUnavailable

logger = logging.getLogger(__name__)

ENV_NO_DOWNLOAD = "AGENTLESS_MCP_NO_DOWNLOAD"
ENV_CACHE_DIR = "TREE_SITTER_LANGUAGE_PACK_CACHE_DIR"
ENV_NO_AUTO_WARM = "AGENTLESS_MCP_NO_AUTO_WARM"

# The background warm stops starting new languages once this much time has
# passed. That is the whole of what it does: `_auto_warm` reads it between
# languages, so it cannot interrupt a fetch already in flight. `pack.prefetch`
# is a Rust extension entry point that takes no timeout at all (checked against
# tree-sitter-language-pack 1.14.3: no timeout parameter in `api.py`,
# `options.py` or `_native.pyi`). What keeps a dead network from holding a
# one-shot CLI exit open is the pair below -- `daemon=True` on the warm thread
# plus AUTO_WARM_JOIN_SECONDS -- and not this constant. Local extraction of the
# full supported set measures ~3s from a cold cache, so this only ever bites on
# a fetch.
AUTO_WARM_DEADLINE_SECONDS = 30.0

# How long a caller waiting for the warm to finish will hold still. Counted
# from the wait rather than from the warm's start, because the callers that
# need it -- a CLI process exiting, a server about to replace its own image --
# can arrive long after the start deadline has lapsed, and a lapsed deadline
# read as "nothing to wait for" is how an extraction gets killed half-written.
# The warm starts no new language past its own deadline, so this only ever
# waits out the extraction already in flight.
AUTO_WARM_JOIN_SECONDS = 10.0

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
    "csharp",
    "hcl",
    "json",
    "kotlin",
    "php",
    "scala",
    "sql",
    "swift",
    "toml",
    "yaml",
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
    "csharp": "class Probe {}\n",
    "go": "package main\n",
    "hcl": 'resource "null_resource" "probe" {}\n',
    "java": "class A {}\n",
    "javascript": "const x = 1;\n",
    "json": '{"x": 1}\n',
    "lua": "local x = 1\n",
    "python": "x = 1\n",
    "ruby": "x = 1\n",
    "scala": "object Probe {}\n",
    "rust": "fn main() {}\n",
    "tsx": "const x = 1;\n",
    "typescript": "const x: number = 1;\n",
    "kotlin": "val x = 1\n",
    "php": "<?php\n$x = 1;\n",
    "sql": "CREATE TABLE probe (id INTEGER);\n",
    "swift": "let x = 1\n",
    "toml": "x = 1\n",
    "yaml": "x: 1\n",
}


@dataclass(frozen=True)
class LanguageCapability:
    """What is actually available for one language, right now.

    ``cached`` and ``warmed`` are two different facts and the remediation
    differs by which one is false. ``cached`` says the pack lists a library for
    this language in the cache directory; ``warmed`` says that library also
    loaded. A row that is cached but not warmed is the one case where the
    advertised remedy is wrong: :func:`_warm_one` skips ``prefetch`` for
    anything already downloaded, so running warmup again fetches nothing and
    the report comes back unchanged.
    """

    name: str
    abi_version: int | None
    pack_version: str
    warmed: bool
    probe_ok: bool
    detail: str = ""
    cached: bool = False

    @property
    def tier(self) -> int:
        """Which support tier this language is in."""
        return 1 if is_tier1(self.name) else 2

    @property
    def unloadable(self) -> bool:
        """True when the grammar is on disk and will not load from there."""
        return self.cached and not self.warmed


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


def auto_warm_disabled() -> bool:
    """True when the environment opts out of the startup background warm."""
    return os.environ.get(ENV_NO_AUTO_WARM, "") not in ("", "0", "false", "False")


@dataclass
class _AutoWarmState:
    """One background warm per process: its thread handle and deadline.

    The handle outlives the thread so a second start is a no-op and the
    "warm in progress" probe stays a liveness check.

    Every read and write of these two fields goes through
    :data:`_AUTO_WARM_LOCK`, with one deliberate exception: ``_auto_warm``
    reads ``deadline`` from inside the warm thread without it. That read must
    stay unlocked, because the thread is started while the lock is held and
    taking it from the target would deadlock. The write that publishes the
    deadline happens before ``start()``, so the thread cannot observe a zero.
    """

    thread: threading.Thread | None = None
    deadline: float = 0.0


# The state and the lock that publishes it. `core/cache.py`'s
# `_AUTO_INDEX_RUNS` is the house pattern for a one-per-process background
# registry and this is the same shape: the check and the set happen together,
# and the thread is started while the lock is still held. Two callers reaching
# the unguarded check at once each saw no warm running and each started one,
# and two concurrent warms write the same grammar into the same cache
# directory -- the truncated-grammar failure `wait_for_auto_warm` exists to
# prevent, reached by a different route.
_AUTO_WARM_LOCK = threading.Lock()
_AUTO_WARM = _AutoWarmState()


def auto_warm_in_progress() -> bool:
    """True while the startup background warm is still running."""
    with _AUTO_WARM_LOCK:
        thread = _AUTO_WARM.thread
    return thread is not None and thread.is_alive()


def start_auto_warm(languages: Sequence[str] | None = None) -> threading.Thread | None:
    """Start one background thread warming cold grammars; never blocks or raises.

    The whole supported set rather than a needs-probe of the repository:
    computing which extensions a repository holds means walking it at
    startup, and the pack fetches one whole platform bundle rather than
    per-language files (module docstring, 2026-08-22), so narrowing the set
    saves no network -- it only leaves languages cold. Warming everything
    matches the explicit warmup's default sweep, and a fresh install needs
    tier 1 warmed too.

    ``AGENTLESS_MCP_NO_DOWNLOAD`` has absolute priority: when it is set,
    nothing starts and today's behavior is exactly preserved. Returns the
    thread when one is running, ``None`` when there is nothing to do.
    """
    if auto_warm_disabled() or no_download_requested():
        return None
    with _AUTO_WARM_LOCK:
        running = _AUTO_WARM.thread
    if running is not None:
        return running

    try:
        warmed = warmed_languages()
    except (pack.Error, RuntimeError, OSError) as error:
        logger.warning("background grammar warm skipped: cache probe failed: %s", error)
        return None
    cold = tuple(name for name in (languages or ALL_LANGUAGES) if name not in warmed)
    if not cold:
        return None

    # Daemon so a closing transport is never held open by a warm; the one-shot
    # CLI pairs the start with wait_for_auto_warm so extraction is not killed
    # mid-write by interpreter shutdown.
    with _AUTO_WARM_LOCK:
        # Re-checked rather than held from the first check: the probe above
        # reads the cache directory, and holding the lock across filesystem
        # work would queue every caller behind it on a cold cache.
        raced = _AUTO_WARM.thread
        if raced is not None:
            return raced
        thread = threading.Thread(
            target=_auto_warm, args=(cold,), name="grammar-auto-warm", daemon=True
        )
        # The deadline is set before the thread starts, so the warm cannot
        # read a zero and stop before it has warmed anything.
        _AUTO_WARM.deadline = time.monotonic() + AUTO_WARM_DEADLINE_SECONDS
        _AUTO_WARM.thread = thread
        # Started under the lock, for the reason `core/cache.py` records: a
        # thread that is registered but not yet started reads as
        # `is_alive() == False`, so a caller probing liveness in that window
        # sees no warm and starts a second one.
        thread.start()
    return thread


def wait_for_auto_warm() -> None:
    """Block until the background warm finishes or the wait budget runs out.

    Without this, process exit would kill the daemon thread partway through a
    cache write, leaving a truncated grammar in a cache other processes read.
    Two callers need it for that reason: the one-shot CLI at its exit, and the
    HTTP server before it replaces its own image on an install update.

    The budget is counted from the wait, not from the warm's start. Anchoring
    it to :data:`AUTO_WARM_DEADLINE_SECONDS` past the start was the same number
    for the CLI's own exit, which happens immediately, and no bound at all for
    any caller reaching here later: a command running longer than the deadline,
    or a server restarting hours in, found the deadline already lapsed and
    skipped the join entirely -- exactly the mid-extraction kill the function
    exists to prevent. The warm stops starting new languages at its own
    deadline, so the only thing this waits out is the one extraction already
    running.
    """
    with _AUTO_WARM_LOCK:
        thread = _AUTO_WARM.thread
    if thread is None:
        return
    # Joined outside the lock. `_auto_warm` never takes it, but a join held
    # under it would block every other caller for the whole join budget.
    thread.join(AUTO_WARM_JOIN_SECONDS)
    if thread.is_alive():
        logger.warning(
            "background grammar warm still running after %.0fs; leaving it rather than "
            "holding the exit open, so its current extraction may be truncated",
            AUTO_WARM_JOIN_SECONDS,
        )


def _auto_warm(names: Sequence[str]) -> None:
    """Warm ``names`` one at a time; one log line, never an exception out."""
    started = time.monotonic()
    warmed: list[str] = []
    degraded: list[str] = []
    stopped = ""
    bundles_before = _bundle_archives()
    try:
        for name in names:
            if time.monotonic() >= _AUTO_WARM.deadline:
                stopped = (
                    f"; deadline of {AUTO_WARM_DEADLINE_SECONDS:.0f}s reached, the rest stay cold"
                )
                break
            report = warmup([name])
            (warmed if report.ok else degraded).append(name)
    except (AgentlessError, pack.Error, RuntimeError, OSError) as error:
        # The contract is one log line and today's labeled-skip behavior,
        # never a crashed process or a traceback mid-session.
        logger.warning("background grammar warm failed: %s", error)
        return

    bundles_after = _bundle_archives()
    notes = ""
    if bundles_before is None or bundles_after is None:
        notes = "; the bundle scan failed, so whether anything was downloaded is unknown"
    elif fetched := sorted(bundles_after - bundles_before):
        notes = f"; fetched {', '.join(fetched)} (sha-256 in name, verified by the pack)"
    if degraded:
        notes += f"; degraded: {', '.join(degraded)} (those languages stay labeled skips)"
    logger.info(
        "background grammar warm: warmed %s in %.1fs (pack %s, cache %s)%s%s",
        ", ".join(warmed) if warmed else "nothing",
        time.monotonic() - started,
        pack_version(),
        cache_dir(),
        notes,
        stopped,
    )


def _bundle_archives() -> frozenset[str] | None:
    """Names of downloaded bundle archives, or None when the scan did not run.

    The pack stores ``bundles/`` and ``manifest.json`` in the parent of the
    libs directory that ``cache_dir()`` names, in both the default and the
    overridden layout (verified 2026-08-22), so the scan starts one level up.

    One non-recursive glob of that one directory. It was ``rglob`` over the
    whole parent, which is not a bounded place: the cache directory is
    operator-supplied through ``TREE_SITTER_LANGUAGE_PACK_CACHE_DIR``, so its
    parent is whatever encloses it, and setting the variable to
    ``$HOME/.grammars`` turned a log detail into two full recursive walks of
    the home directory at process start.

    None rather than an empty set when the scan fails, because the caller
    subtracts two of these: an empty set would read as "nothing was fetched"
    for a scan that never ran. ``Path.glob`` itself swallows a missing or
    unreadable directory and yields nothing, so what the guard catches is
    :func:`cache_dir` failing to answer.
    """
    try:
        bundles = Path(cache_dir()).parent / "bundles"
        return frozenset(path.name for path in bundles.glob("*.tar.zst"))
    except (pack.Error, RuntimeError, OSError):
        return None


def warmed_languages() -> frozenset[str]:
    """Return the languages loadable from the local cache without a fetch."""
    _configured_cache_dir()
    return frozenset(pack.downloaded_languages())


def unavailable_reason(name: str) -> str:
    """The labeled-skip reason for a grammar that is not warmed.

    One home for the wording so the index report and a live scan describe the
    file identically. While the startup background warm is still running the
    remediation is to wait, not to run a second warm into the same cache.
    """
    if auto_warm_in_progress():
        return (
            f"language '{name}' not warmed: a background warm is in progress, "
            "retry shortly or run agentless-mcp warmup"
        )
    return f"language '{name}' not warmed: run agentless-mcp warmup"


def unloadable_reason(name: str, cause: str) -> str:
    """The reason for a grammar the pack has cached but cannot load.

    Kept apart from :func:`unavailable_reason` because the usual remedy is a
    no-op here: :func:`_warm_one` fetches nothing for a language
    ``downloaded_languages()`` already lists, so a reader told to run warmup
    runs it, nothing is fetched, and the report comes back word for word the
    same. The library on disk has to go before a fetch will replace it.
    """
    return (
        f"language '{name}' is cached in {cache_dir()} but failed to load: {cause}. "
        "Warmup will not refetch it: delete the cached library for this language, "
        "or reinstall tree-sitter-language-pack."
    )


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
        raise LanguageUnavailable(unavailable_reason(name))

    try:
        return pack.get_language(name)
    except (pack.Error, RuntimeError) as exc:
        raise LanguageUnavailable(unloadable_reason(name, str(exc))) from exc


def get_parser(name: str) -> Parser:
    """Return a fresh parser for ``name``. Never fetches.

    Fresh rather than memoized, deliberately. A ``Parser`` carries mutable
    state across ``parse`` calls, and this function is reached from four
    parse sites while the background index thread is running -- so one shared
    instance is one object being driven by two threads at once. It is safe
    today only because py-tree-sitter 0.26 holds the GIL for the whole of
    ``parse``; the ``>=0.25,<0.27`` pin does not promise that, and a
    free-threaded build removes it.

    The memo bought nothing to weigh against that. Measured on this machine:
    constructing a ``Parser`` costs 0.2 us against 405 us to parse 4.6 KB,
    three orders of magnitude apart, and the grammar itself -- the expensive,
    immutable half -- is still shared through :func:`get_language`.
    :func:`_probe` already built one per call, which is the codebase's own
    evidence that nothing depended on the identity.
    """
    return Parser(get_language(name))


def warmup(
    languages: Sequence[str] | None = None,
    *,
    no_download: bool = False,
) -> WarmupReport:
    """Fetch, load and probe-parse each language; report per language.

    This is the only function that downloads. A language that fails to fetch,
    load or probe is reported as degraded; the run continues. The one case
    this function raises deliberately is a refusal: a fetch is required but
    downloads are forbidden, which is a configuration decision the caller must
    see, not a degraded row they might page past.

    A cache directory that cannot be read or written raises ``OSError`` out of
    the pack on top of that. It is the whole local cache failing rather than
    one language, so a caller that must not fail -- :func:`_auto_warm` --
    catches it around this call.
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
            raise AgentlessError(message)
        try:
            # Unbounded, deliberately. `pack.prefetch` takes no timeout, and
            # both ways to impose one -- killing a worker thread or a
            # subprocess partway through -- leave a half-written file in a
            # cache other processes read, which is the failure
            # `wait_for_auto_warm` exists to prevent. Its two callers are
            # bounded where the bound is safe: the background warm runs on a
            # daemon thread that process exit joins for AUTO_WARM_JOIN_SECONDS,
            # and `agentless-mcp warmup` is a foreground command a person typed
            # whose whole job is to download.
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
    """Load a warmed grammar and parse its probe sample.

    The cached state is read before the load, not derived from it: a grammar
    the pack lists as downloaded and then refuses to load is a different
    condition from one that was never fetched, and only the first fact
    separates them once the load has failed.
    """
    cached = name in warmed_languages()
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
            cached=cached,
        )

    probe_ok, detail = _probe(name, language)
    return LanguageCapability(
        name=name,
        abi_version=language.abi_version,
        pack_version=version,
        warmed=True,
        probe_ok=probe_ok,
        detail=detail,
        cached=True,
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
