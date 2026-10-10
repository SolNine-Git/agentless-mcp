"""Checks that compare one file's whole text before and after a change.

They take typed text pairs rather than :class:`agentless_mcp.core.patchlint.RepoFacts`,
so ``lint`` can run them without the repository scan the patch checks need.
``clone_delta`` also takes a :class:`CloneIndex` of the repository's source
files, which the caller reads. Like every lint check they are advisory: nothing
here blocks a change. This module opens no file.
"""

import difflib
from collections import Counter, defaultdict
from collections.abc import Callable, Collection, Hashable, Mapping, Sequence
from dataclasses import dataclass, field, fields
from functools import partial
from pathlib import PurePosixPath
from types import MappingProxyType
from typing import Any, Final, TypeVar

from agentless_mcp.core import clones, complexity, contracts
from agentless_mcp.core.clones import Block, FileBlocks, place
from agentless_mcp.core.complexity import Unit
from agentless_mcp.core.contracts import Hit
from agentless_mcp.core.extractor import TreeSitterExtractor
from agentless_mcp.core.patchlint import MAX_DUPLICATE_SITES, Finding, Severity
from agentless_mcp.core.projectconfig import CONFIG_FILENAME
from agentless_mcp.prompts import MESSAGES
from agentless_mcp.util.errors import LanguageUnavailable, UnmeasurableSource

CHECK_COMPLEXITY_DELTA = "complexity_delta"
CHECK_CONTRACT_DRIFT = "contract_drift"
CHECK_CLONE_DELTA = "clone_delta"

# How many unindexed files the one clone_delta coverage row names.
MAX_NAMED_GAPS: Final = 10

MODULE_UNIT = "<module>"
GAP_NO_BOUNDARY_GLOBS = "no_boundary_globs"
GAP_NO_RULES = "no_rules_for_language"

MATCH_BODY = "body"
MATCH_NAME = "name"
MATCH_COLLISION = "collision"
MATCH_NEW = "new"

_Row = TypeVar("_Row")
_Read = TypeVar("_Read")


@dataclass(frozen=True)
class TextPair:
    """One file's whole text before and after a change, ``None`` for a side that does not exist."""

    path: str
    old: str | None
    new: str | None


@dataclass(frozen=True)
class UnitDelta:
    """The numbers behind one unit's finding, with ``None`` for the old side of a new unit."""

    unit: str
    match: str
    cc_old: int | None
    cc_new: int
    sloc_old: int | None
    sloc_new: int
    threshold: int

    def as_dict(self) -> dict[str, Any]:
        """Return the JSON form of this delta."""
        return {
            "unit": self.unit,
            "match": self.match,
            "cc_old": self.cc_old,
            "cc_new": self.cc_new,
            "sloc_old": self.sloc_old,
            "sloc_new": self.sloc_new,
            "threshold": self.threshold,
        }


@dataclass(frozen=True)
class DriftKey:
    """One contract hit's identity: the rule, the unit that holds it, and its normalized text."""

    rule: str
    unit: str
    capture: str

    def as_dict(self) -> dict[str, Any]:
        """Return the JSON form of this key."""
        return {"rule": self.rule, "unit": self.unit, "capture": self.capture}


@dataclass(frozen=True)
class DriftGap:
    """Why contract_drift did not check a change by design, as a reason a caller can match."""

    reason: str
    language: str | None = None

    def as_dict(self) -> dict[str, Any]:
        """Return the JSON form of this gap, with ``language`` only when it names one."""
        document: dict[str, Any] = {"reason": self.reason}
        if self.language is not None:
            document["language"] = self.language
        return document


@dataclass(frozen=True)
class PairFinding:
    """A pair-check finding, and the machine-readable detail behind it when it has one."""

    finding: Finding
    detail: UnitDelta | DriftKey | DriftGap | None = None


@dataclass(frozen=True)
class UnitState:
    """One unit of a new text with complexity above the threshold."""

    path: str
    unit: str
    line: int
    cc: int
    sloc: int

    def as_dict(self) -> dict[str, Any]:
        """Return the JSON form of this row."""
        return {
            "path": self.path,
            "unit": self.unit,
            "line": self.line,
            "cc": self.cc,
            "sloc": self.sloc,
        }


@dataclass(frozen=True)
class UnitChange:
    """One unit a change added or altered, with ``None`` for the old side of a new unit."""

    path: str
    unit: str
    line: int
    match: str
    cc_old: int | None
    cc_new: int
    sloc_old: int | None
    sloc_new: int

    def as_dict(self) -> dict[str, Any]:
        """Return the JSON form of this row."""
        return {
            "path": self.path,
            "unit": self.unit,
            "line": self.line,
            "match": self.match,
            "cc_old": self.cc_old,
            "cc_new": self.cc_new,
            "sloc_old": self.sloc_old,
            "sloc_new": self.sloc_new,
        }


@dataclass(frozen=True)
class HitState:
    """One contract hit in a new text, named by the unit that holds it in that text."""

    path: str
    rule: str
    unit: str
    capture: str

    def as_dict(self) -> dict[str, Any]:
        """Return the JSON form of this row."""
        return {"path": self.path, "rule": self.rule, "unit": self.unit, "capture": self.capture}


@dataclass(frozen=True)
class Observed:
    """The state of the new text; a field is ``None`` when its check measured no file."""

    units_over_threshold: tuple[UnitState, ...] | None = None
    units_changed: tuple[UnitChange, ...] | None = None
    contract_keys: tuple[HitState, ...] | None = None

    def merged(self, other: "Observed") -> "Observed":
        """Return this state with each field it does not hold taken from ``other``."""
        return Observed(
            **{
                item.name: _first(getattr(self, item.name), getattr(other, item.name))
                for item in fields(self)
            }
        )

    def as_dict(self) -> dict[str, Any]:
        """Return the JSON form, with only the fields a check measured."""
        return {
            item.name: [row.as_dict() for row in rows]
            for item in fields(self)
            if (rows := getattr(self, item.name)) is not None
        }


@dataclass(frozen=True)
class PairOutcome:
    """A pair check's findings, and the state it observed in the new text."""

    findings: tuple[PairFinding, ...]
    observed: Observed = field(default_factory=Observed)


def _no_files() -> Mapping[str, FileBlocks]:
    return MappingProxyType({})


def _no_reasons() -> Mapping[str, str]:
    return MappingProxyType({})


@dataclass(frozen=True)
class CloneIndex:
    """The repository files clone_delta indexed, and why each other file was not, by path."""

    files: Mapping[str, FileBlocks] = field(default_factory=_no_files)
    unindexed: Mapping[str, str] = field(default_factory=_no_reasons)
    unlisted: str | None = None


@dataclass(frozen=True)
class _Indexed:
    path: str
    old: tuple[Block, ...]
    new: tuple[Block, ...]
    unchanged: Mapping[int, int]


@dataclass(frozen=True)
class _Gaps:
    check: str
    old: str
    new: str


@dataclass(frozen=True)
class _Measured:
    pair: TextPair
    language: str
    old: tuple[Unit, ...]
    new: tuple[Unit, ...]


@dataclass(frozen=True)
class _Match:
    old: Unit | None
    new: Unit
    via: str


_COMPLEXITY_GAPS = _Gaps(
    CHECK_COMPLEXITY_DELTA,
    MESSAGES.complexity_old_not_measured,
    MESSAGES.complexity_new_not_measured,
)
_CONTRACT_GAPS = _Gaps(
    CHECK_CONTRACT_DRIFT,
    MESSAGES.contract_old_not_measured,
    MESSAGES.contract_new_not_measured,
)


def pair_language(path: str) -> str | None:
    """Return the language the pair checks read ``path`` as, or ``None`` for a file they skip."""
    language = TreeSitterExtractor.SUPPORTED_EXTENSIONS.get(PurePosixPath(path).suffix)
    return language if language in complexity.RULES else None


def complexity_delta(pairs: Sequence[TextPair], threshold: int) -> PairOutcome:
    """Report each unit a change grew above ``threshold``, took over it, or added over it."""
    measured, gaps = _each_pair(pairs, partial(_measure, gaps=_COMPLEXITY_GAPS))
    matches = _match_run(measured)
    found = [
        finding for match in matches if (finding := _unit_finding(match, threshold)) is not None
    ]
    observed = (
        Observed(
            units_over_threshold=_over_threshold(measured, threshold),
            units_changed=_changed(matches),
        )
        if measured
        else Observed()
    )
    return PairOutcome(_sorted([*gaps, *found]), observed)


def contract_drift(pairs: Sequence[TextPair], globs: Sequence[str]) -> PairOutcome:
    """Report each contract-rule hit a change adds to a file that one of ``globs`` matches."""
    if not globs:
        return PairOutcome(_no_boundary_rows(pairs))
    admitted, rows = _admitted(pairs, globs)
    measured, gaps = _each_pair(admitted, partial(_measure, gaps=_CONTRACT_GAPS))
    found, keys = _new_hits(measured, _match_run(measured))
    observed = Observed(contract_keys=keys) if measured else Observed()
    return PairOutcome(_sorted([*rows, *gaps, *found]), observed)


def clone_index(texts: Mapping[str, str], unread: Mapping[str, str]) -> CloneIndex:
    """Index the clone blocks of each tier-1 text, keeping each file read or indexed in vain."""
    files: dict[str, FileBlocks] = {}
    unindexed = dict(unread)
    for path, text in sorted(texts.items()):
        try:
            files[path] = clones.file_blocks(text, path, _source_language(path))
        except (UnmeasurableSource, LanguageUnavailable) as exc:
            unindexed[path] = str(exc)
    return CloneIndex(
        files=MappingProxyType(files), unindexed=MappingProxyType(dict(sorted(unindexed.items())))
    )


def clone_delta(
    pairs: Sequence[TextPair], index: CloneIndex, reported: Collection[tuple[str, int]]
) -> PairOutcome:
    """Report each block a change adds that has a Type-2 copy elsewhere once the change is in.

    ``index`` holds the repository as the working tree has it, and every index
    file a pair names is replaced by that pair. ``reported`` holds the
    ``(path, line)`` of each function ``near_duplicates`` reported in the same run.
    """
    indexed, rows = _each_pair(pairs, _index_pair)
    changed = {pair.path for pair in pairs}
    instances = _after_instances(indexed, index, changed)
    copies = [
        block for block in _added(indexed) if len(instances[block.key]) >= clones.MIN_INSTANCES
    ]
    found = [_clone_finding(block, instances[block.key]) for block in _outermost(copies, reported)]
    return PairOutcome(_sorted([*_index_rows(index, changed), *rows, *found]))


def _first(value: _Row | None, fallback: _Row | None) -> _Row | None:
    return fallback if value is None else value


def _each_pair(
    pairs: Sequence[TextPair], read: Callable[[TextPair, str], _Read | PairFinding]
) -> tuple[list[_Read], list[PairFinding]]:
    if len({pair.path for pair in pairs}) != len(pairs):
        message = "each path may appear in only one text pair of a run"
        raise ValueError(message)
    outcomes: list[_Read] = []
    rows: list[PairFinding] = []
    for pair in pairs:
        language = pair_language(pair.path)
        if language is None:
            continue
        outcome = read(pair, language)
        if isinstance(outcome, PairFinding):
            rows.append(outcome)
        else:
            outcomes.append(outcome)
    return outcomes, rows


def _measure(pair: TextPair, language: str, gaps: _Gaps) -> _Measured | PairFinding:
    old = _units(pair.old, pair.path, language, gaps.old, gaps.check)
    if isinstance(old, PairFinding):
        return old
    new = _units(pair.new, pair.path, language, gaps.new, gaps.check)
    if isinstance(new, PairFinding):
        return new
    return _Measured(pair, language, old, new)


def _units(
    text: str | None, path: str, language: str, gap: str, check: str
) -> tuple[Unit, ...] | PairFinding:
    if text is None:
        return ()
    try:
        return complexity.measure(text, path, language)
    except (UnmeasurableSource, LanguageUnavailable) as exc:
        return _not_checked(
            check, path, gap.format(path=path, reason=exc), evidence=type(exc).__name__
        )


def _not_checked(
    check: str, path: str, message: str, evidence: str, gap: DriftGap | None = None
) -> PairFinding:
    return PairFinding(
        Finding(
            check=check,
            severity=Severity.NOT_CHECKED,
            message=message,
            path=path,
            line=0,
            evidence=evidence,
        ),
        gap,
    )


def _match_run(measured: Sequence[_Measured]) -> list[_Match]:
    matches: list[_Match] = []
    old_left: list[Unit] = []
    new_left: list[Unit] = []
    for item in measured:
        paired, olds, news = _body_pass(item.old, item.new)
        matches.extend(paired)
        old_left.extend(olds)
        new_left.extend(news)

    # Each file pairs its own units first, so a copy beside its unchanged
    # original is new in the other file, and a unit moved between files is not.
    moved, old_rest, new_rest = _body_pass(old_left, new_left)
    matches.extend(moved)

    old_by_file, new_by_file = _by_file(old_rest), _by_file(new_rest)
    for item in measured:
        named, unpaired = _name_and_collision_passes(
            old_by_file.get(item.pair.path, []), new_by_file.get(item.pair.path, [])
        )
        matches.extend(named)
        matches.extend(_Match(None, unit, MATCH_NEW) for unit in unpaired)
    return matches


def _body_pass(
    old_units: Sequence[Unit], new_units: Sequence[Unit]
) -> tuple[list[_Match], list[Unit], list[Unit]]:
    waiting: dict[str, list[Unit]] = defaultdict(list)
    for unit in old_units:
        waiting[unit.body_key].append(unit)

    matches: list[_Match] = []
    new_left: list[Unit] = []
    for unit in new_units:
        queue = waiting.get(unit.body_key)
        if queue:
            matches.append(_Match(queue.pop(0), unit, MATCH_BODY))
        else:
            new_left.append(unit)

    old_left = [unit for queue in waiting.values() for unit in queue]
    return matches, old_left, new_left


def _name_and_collision_passes(
    old_left: Sequence[Unit], new_left: Sequence[Unit]
) -> tuple[list[_Match], list[Unit]]:
    old_named = _by_qualname(old_left)
    matches: list[_Match] = []
    unpaired: list[Unit] = []
    for qualname, news in _by_qualname(new_left).items():
        olds = old_named.get(qualname, [])
        if len(olds) == 1 and len(news) == 1:
            matches.append(_Match(olds[0], news[0], MATCH_NAME))
            continue
        # A multiset by rank: ordinals shift when a same-name unit is inserted
        # above another, and a ranking does not.
        ranked_old = sorted(olds, key=_rank)
        ranked_new = sorted(news, key=_rank)
        matches.extend(
            _Match(old, new, MATCH_COLLISION)
            for old, new in zip(ranked_old, ranked_new, strict=False)
        )
        unpaired.extend(ranked_new[len(ranked_old) :])
    return matches, unpaired


def _by_qualname(units: Sequence[Unit]) -> dict[str, list[Unit]]:
    grouped: dict[str, list[Unit]] = {}
    for unit in units:
        grouped.setdefault(unit.qualname, []).append(unit)
    return grouped


def _by_file(units: Sequence[Unit]) -> dict[str, list[Unit]]:
    grouped: dict[str, list[Unit]] = {}
    for unit in units:
        grouped.setdefault(unit.path, []).append(unit)
    return grouped


def _rank(unit: Unit) -> tuple[int, int]:
    return (-unit.cc, unit.start_byte)


def _unit_finding(match: _Match, threshold: int) -> PairFinding | None:
    old, new = match.old, match.new
    if old is None:
        if new.cc <= threshold:
            return None
        template, evidence = MESSAGES.complexity_new_unit, MESSAGES.complexity_new_unit_evidence
    elif old.cc > threshold and new.cc > old.cc:
        template, evidence = MESSAGES.complexity_grew, MESSAGES.complexity_evidence
    elif old.cc <= threshold < new.cc:
        template, evidence = MESSAGES.complexity_crossed, MESSAGES.complexity_evidence
    else:
        return None

    detail = UnitDelta(
        unit=new.qualname,
        match=match.via,
        cc_old=None if old is None else old.cc,
        cc_new=new.cc,
        sloc_old=None if old is None else old.sloc,
        sloc_new=new.sloc,
        threshold=threshold,
    )
    numbers = {
        "qualname": new.qualname,
        "old_cc": detail.cc_old,
        "new_cc": new.cc,
        "old_sloc": detail.sloc_old,
        "new_sloc": new.sloc,
        "threshold": threshold,
    }
    finding = Finding(
        check=CHECK_COMPLEXITY_DELTA,
        severity=Severity.WARNING,
        message=template.format(**numbers),
        path=new.path,
        line=new.start_line,
        evidence=evidence.format(**numbers),
    )
    return PairFinding(finding, detail)


def _over_threshold(measured: Sequence[_Measured], threshold: int) -> tuple[UnitState, ...]:
    rows = [
        UnitState(unit.path, unit.qualname, unit.start_line, unit.cc, unit.sloc)
        for item in measured
        for unit in item.new
        if unit.cc > threshold
    ]
    return tuple(sorted(rows, key=_row_order))


def _changed(matches: Sequence[_Match]) -> tuple[UnitChange, ...]:
    rows = [
        UnitChange(
            path=match.new.path,
            unit=match.new.qualname,
            line=match.new.start_line,
            match=match.via,
            cc_old=None if match.old is None else match.old.cc,
            cc_new=match.new.cc,
            sloc_old=None if match.old is None else match.old.sloc,
            sloc_new=match.new.sloc,
        )
        for match in matches
        if match.via != MATCH_BODY
    ]
    return tuple(sorted(rows, key=_row_order))


def _row_order(row: UnitState | UnitChange) -> tuple[str, int, str]:
    return (row.path, row.line, row.unit)


def _sorted(findings: Sequence[PairFinding]) -> tuple[PairFinding, ...]:
    return tuple(sorted(findings, key=_order))


def _order(found: PairFinding) -> tuple[str, int, str, str]:
    finding = found.finding
    return (finding.path, finding.line, finding.message, finding.evidence)


def _no_boundary_rows(pairs: Sequence[TextPair]) -> tuple[PairFinding, ...]:
    if not pairs:
        return ()
    message = MESSAGES.contract_no_boundary.format(config=CONFIG_FILENAME)
    evidence = MESSAGES.contract_no_boundary_evidence
    gap = DriftGap(GAP_NO_BOUNDARY_GLOBS)
    return (_not_checked(CHECK_CONTRACT_DRIFT, "", message, evidence, gap),)


def _admitted(
    pairs: Sequence[TextPair], globs: Sequence[str]
) -> tuple[list[TextPair], list[PairFinding]]:
    admitted: list[TextPair] = []
    rows: list[PairFinding] = []
    for pair in pairs:
        language = pair_language(pair.path)
        if language is None or not contracts.in_boundary(pair.path, globs):
            continue
        if contracts.rules_for(language):
            admitted.append(pair)
        elif pair.new is not None:
            message = MESSAGES.contract_no_rules.format(language=language, path=pair.path)
            gap = DriftGap(GAP_NO_RULES, language)
            rows.append(_not_checked(CHECK_CONTRACT_DRIFT, pair.path, message, language, gap))
    return admitted, rows


def _new_hits(
    measured: Sequence[_Measured], matches: Sequence[_Match]
) -> tuple[list[PairFinding], tuple[HitState, ...]]:
    # A new unit that `_match_run` paired keeps its old identity, so a rename or
    # a move to another file of the run moves no hit.
    identity = {
        (match.new.path, match.new.start_byte): match.new if match.old is None else match.old
        for match in matches
    }
    carried = _carried(measured)
    fresh: list[PairFinding] = []
    states: list[HitState] = []
    for item in measured:
        if item.pair.new is None:
            continue
        for hit in contracts.hits(item.pair.new, item.language, item.new):
            owner = None if hit.unit is None else identity[(hit.unit.path, hit.unit.start_byte)]
            states.append(HitState(item.pair.path, hit.rule.id, _qualname(hit.unit), hit.capture))
            key = _key(hit, owner, item.pair.path)
            if carried[key]:
                carried[key] -= 1
            else:
                drift = DriftKey(rule=hit.rule.id, unit=_qualname(owner), capture=hit.capture)
                fresh.append(_hit_finding(item.pair.path, hit, drift))
    return fresh, tuple(sorted(states, key=lambda state: state.path))


def _carried(measured: Sequence[_Measured]) -> Counter[tuple[str, str, str, str]]:
    return Counter(
        _key(hit, hit.unit, item.pair.path)
        for item in measured
        if item.pair.old is not None
        for hit in contracts.hits(item.pair.old, item.language, item.old)
    )


def _key(hit: Hit, owner: Unit | None, path: str) -> tuple[str, str, str, str]:
    where = path if owner is None else owner.path
    return (hit.rule.id, where, _qualname(owner), hit.capture)


def _qualname(unit: Unit | None) -> str:
    return MODULE_UNIT if unit is None else unit.qualname


def _hit_finding(path: str, hit: Hit, key: DriftKey) -> PairFinding:
    parts = {"rule": key.rule, "unit": key.unit, "capture": key.capture}
    finding = Finding(
        check=CHECK_CONTRACT_DRIFT,
        severity=Severity(hit.rule.severity),
        message=MESSAGES.contract_new_hit.format(why=hit.rule.message, **parts),
        path=path,
        line=hit.line,
        evidence=MESSAGES.contract_evidence.format(**parts),
    )
    return PairFinding(finding, key)


BlockKey = Callable[[Block], Hashable]


def _source_language(path: str) -> str:
    language = pair_language(path)
    if language is None:
        message = f"{path} is not a tier-1 source file, so clone_delta cannot index it"
        raise ValueError(message)
    return language


def _index_pair(pair: TextPair, language: str) -> _Indexed | PairFinding:
    old = _side_blocks(pair.old, pair.path, language, MESSAGES.clone_old_not_indexed)
    if isinstance(old, PairFinding):
        return old
    new = _side_blocks(pair.new, pair.path, language, MESSAGES.clone_new_not_indexed)
    if isinstance(new, PairFinding):
        return new
    return _Indexed(pair.path, old, new, _unchanged_lines(pair.old, pair.new))


def _side_blocks(
    text: str | None, path: str, language: str, gap: str
) -> tuple[Block, ...] | PairFinding:
    if text is None:
        return ()
    try:
        return clones.file_blocks(text, path, language).blocks
    except (UnmeasurableSource, LanguageUnavailable) as exc:
        message = gap.format(path=path, reason=exc)
        return _not_checked(CHECK_CLONE_DELTA, path, message, evidence=type(exc).__name__)


def _unchanged_lines(old: str | None, new: str | None) -> Mapping[int, int]:
    # Split on "\n" alone, as tree-sitter numbers rows.
    if old is None or new is None:
        return MappingProxyType({})
    matcher = difflib.SequenceMatcher(None, old.split("\n"), new.split("\n"))
    return MappingProxyType(
        {
            first + offset + 1: second + offset + 1
            for first, second, size in matcher.get_matching_blocks()
            for offset in range(size)
        }
    )


def _after_instances(
    indexed: Sequence[_Indexed], index: CloneIndex, changed: Collection[str]
) -> dict[str, list[Block]]:
    instances: dict[str, list[Block]] = defaultdict(list)
    for path, unchanged in index.files.items():
        if path not in changed:
            for block in unchanged.blocks:
                instances[block.key].append(block)
    for item in indexed:
        for block in item.new:
            instances[block.key].append(block)
    return instances


def _exact(block: Block) -> Hashable:
    return block.exact


def _type2(block: Block) -> Hashable:
    return block.key


def _added(indexed: Sequence[_Indexed]) -> list[Block]:
    # Each file pairs its own blocks first, so a copy beside its original is the
    # added one, and only then do the leftovers pair across files as moves.
    old_left: list[Block] = []
    new_left: list[Block] = []
    for item in indexed:
        olds, news = _within_file(item)
        old_left.extend(olds)
        new_left.extend(news)
    olds, news = sorted(old_left, key=place), sorted(new_left, key=place)
    for key in (_exact, _type2):
        olds, news = _pair_by(olds, news, key, key)
    return news


def _within_file(item: _Indexed) -> tuple[list[Block], list[Block]]:
    # A block on lines the diff left in place pairs with itself before any copy of it can.
    olds, news = _pair_by(
        item.old,
        item.new,
        lambda block: (item.unchanged.get(block.start_line), block.exact),
        lambda block: (block.start_line, block.exact),
    )
    for key in (_exact, _type2):
        olds, news = _pair_by(olds, news, key, key)
    return olds, news


def _pair_by(
    old: Sequence[Block], new: Sequence[Block], old_key: BlockKey, new_key: BlockKey
) -> tuple[list[Block], list[Block]]:
    waiting: dict[Hashable, list[Block]] = defaultdict(list)
    for block in old:
        waiting[old_key(block)].append(block)
    new_left: list[Block] = []
    for block in new:
        queue = waiting.get(new_key(block))
        if queue:
            queue.pop(0)
        else:
            new_left.append(block)
    old_left = sorted((block for queue in waiting.values() for block in queue), key=place)
    return old_left, new_left


def _outermost(copies: Sequence[Block], reported: Collection[tuple[str, int]]) -> list[Block]:
    # A copy inside a reported copy, or inside a function near_duplicates
    # already reported, folds into it, so one pasted function is one finding.
    kept: list[Block] = []
    outer: Block | None = None
    for block in sorted(copies, key=lambda copy: (copy.path, copy.start_line, -copy.end_line)):
        if outer is not None and _within(block, outer):
            continue
        outer = block
        if not (block.function and (block.path, block.start_line) in reported):
            kept.append(block)
    return kept


def _within(block: Block, outer: Block) -> bool:
    return (
        block.path == outer.path
        and outer.start_line <= block.start_line
        and block.end_line <= outer.end_line
    )


def _clone_finding(block: Block, instances: Sequence[Block]) -> PairFinding:
    others = sorted((other for other in instances if other is not block), key=place)
    sites = [f"{other.path}:{other.start_line}" for other in others]
    finding = Finding(
        check=CHECK_CLONE_DELTA,
        severity=Severity.ADVISORY,
        message=MESSAGES.clone_new_copy.format(
            start=block.start_line, end=block.end_line, count=len(instances)
        ),
        path=block.path,
        line=block.start_line,
        evidence=MESSAGES.clone_new_copy_evidence.format(sites=_listed(sites, MAX_DUPLICATE_SITES)),
    )
    return PairFinding(finding)


def _listed(items: Sequence[str], limit: int) -> str:
    shown = ", ".join(items[:limit])
    if len(items) <= limit:
        return shown
    return MESSAGES.clone_and_more.format(listed=shown, count=len(items) - limit)


def _index_rows(index: CloneIndex, changed: Collection[str]) -> list[PairFinding]:
    rows: list[PairFinding] = []
    if index.unlisted is not None:
        message = MESSAGES.clone_index_unlisted.format(reason=index.unlisted)
        rows.append(_not_checked(CHECK_CLONE_DELTA, "", message, evidence=index.unlisted))
    gaps = [(path, reason) for path, reason in index.unindexed.items() if path not in changed]
    if gaps:
        total = len(gaps) + sum(1 for path in index.files if path not in changed)
        message = MESSAGES.clone_index_gaps.format(
            count=len(gaps), total=total, paths=_listed([path for path, _ in gaps], MAX_NAMED_GAPS)
        )
        first, reason = gaps[0]
        evidence = MESSAGES.clone_index_gaps_evidence.format(path=first, reason=reason)
        rows.append(_not_checked(CHECK_CLONE_DELTA, "", message, evidence=evidence))
    return rows
