"""Checks that compare one file's whole text before and after a change.

They take typed text pairs rather than :class:`agentless_mcp.core.patchlint.RepoFacts`,
so ``lint`` can run them without scanning the repository. Like every lint check
they are advisory: nothing here blocks a change. This module opens no file.
"""

from collections import defaultdict
from collections.abc import Sequence
from dataclasses import dataclass
from pathlib import PurePosixPath
from typing import Any

from agentless_mcp.core import complexity
from agentless_mcp.core.complexity import Unit
from agentless_mcp.core.extractor import TreeSitterExtractor
from agentless_mcp.core.patchlint import Finding, Severity
from agentless_mcp.prompts import MESSAGES
from agentless_mcp.util.errors import LanguageUnavailable, UnmeasurableSource

CHECK_COMPLEXITY_DELTA = "complexity_delta"

MATCH_BODY = "body"
MATCH_NAME = "name"
MATCH_COLLISION = "collision"
MATCH_NEW = "new"


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
class PairFinding:
    """A pair-check finding, and its unit numbers when it is about one unit."""

    finding: Finding
    detail: UnitDelta | None = None


@dataclass(frozen=True)
class _Match:
    old: Unit | None
    new: Unit
    via: str


def pair_language(path: str) -> str | None:
    """Return the language the pair checks read ``path`` as, or ``None`` for a file they skip."""
    language = TreeSitterExtractor.SUPPORTED_EXTENSIONS.get(PurePosixPath(path).suffix)
    return language if language in complexity.RULES else None


def complexity_delta(pairs: Sequence[TextPair], threshold: int) -> tuple[PairFinding, ...]:
    """Report each unit a change grew above ``threshold``, took over it, or added over it."""
    findings: list[PairFinding] = []
    for pair in pairs:
        language = pair_language(pair.path)
        if language is None or pair.new is None:
            continue
        old_units = _units(pair.old, pair.path, language, MESSAGES.complexity_old_not_measured)
        if isinstance(old_units, PairFinding):
            findings.append(old_units)
            continue
        new_units = _units(pair.new, pair.path, language, MESSAGES.complexity_new_not_measured)
        if isinstance(new_units, PairFinding):
            findings.append(new_units)
            continue
        for match in _match(old_units, new_units):
            found = _unit_finding(pair.path, match, threshold)
            if found is not None:
                findings.append(found)
    return tuple(sorted(findings, key=_order))


def _units(text: str | None, path: str, language: str, gap: str) -> tuple[Unit, ...] | PairFinding:
    if text is None:
        return ()
    try:
        return complexity.measure(text, path, language)
    except (UnmeasurableSource, LanguageUnavailable) as exc:
        return PairFinding(
            Finding(
                check=CHECK_COMPLEXITY_DELTA,
                severity=Severity.NOT_CHECKED,
                message=gap.format(path=path, reason=exc),
                path=path,
                line=0,
                evidence=type(exc).__name__,
            )
        )


def _match(old_units: Sequence[Unit], new_units: Sequence[Unit]) -> list[_Match]:
    by_body, old_left, new_left = _body_pass(old_units, new_units)
    by_name, new_unpaired = _name_and_collision_passes(old_left, new_left)
    return [*by_body, *by_name, *(_Match(None, unit, MATCH_NEW) for unit in new_unpaired)]


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


def _rank(unit: Unit) -> tuple[int, int]:
    return (-unit.cc, unit.start_byte)


def _unit_finding(path: str, match: _Match, threshold: int) -> PairFinding | None:
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
        path=path,
        line=new.start_line,
        evidence=evidence.format(**numbers),
    )
    return PairFinding(finding, detail)


def _order(found: PairFinding) -> tuple[str, int, str, str]:
    finding = found.finding
    return (finding.path, finding.line, finding.message, finding.evidence)
