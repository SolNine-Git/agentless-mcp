"""Checks that compare one file's whole text before and after a change.

They take typed text pairs rather than :class:`agentless_mcp.core.patchlint.RepoFacts`,
so ``lint`` can run them without scanning the repository. Like every lint check
they are advisory: nothing here blocks a change. This module opens no file.
"""

from collections import Counter, defaultdict
from collections.abc import Sequence
from dataclasses import dataclass
from pathlib import PurePosixPath
from typing import Any

from agentless_mcp.core import complexity, contracts
from agentless_mcp.core.complexity import Unit
from agentless_mcp.core.contracts import Hit
from agentless_mcp.core.extractor import TreeSitterExtractor
from agentless_mcp.core.patchlint import Finding, Severity
from agentless_mcp.core.projectconfig import CONFIG_FILENAME
from agentless_mcp.prompts import MESSAGES
from agentless_mcp.util.errors import LanguageUnavailable, UnmeasurableSource

CHECK_COMPLEXITY_DELTA = "complexity_delta"
CHECK_CONTRACT_DRIFT = "contract_drift"

MODULE_UNIT = "<module>"
GAP_NO_BOUNDARY_GLOBS = "no_boundary_globs"
GAP_NO_RULES = "no_rules_for_language"

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
        old_units = _units(
            pair.old,
            pair.path,
            language,
            MESSAGES.complexity_old_not_measured,
            CHECK_COMPLEXITY_DELTA,
        )
        if isinstance(old_units, PairFinding):
            findings.append(old_units)
            continue
        new_units = _units(
            pair.new,
            pair.path,
            language,
            MESSAGES.complexity_new_not_measured,
            CHECK_COMPLEXITY_DELTA,
        )
        if isinstance(new_units, PairFinding):
            findings.append(new_units)
            continue
        for match in _match(old_units, new_units):
            found = _unit_finding(pair.path, match, threshold)
            if found is not None:
                findings.append(found)
    return tuple(sorted(findings, key=_order))


def contract_drift(pairs: Sequence[TextPair], globs: Sequence[str]) -> tuple[PairFinding, ...]:
    """Report each contract-rule hit a change adds to a file that one of ``globs`` matches."""
    if not globs:
        if not pairs:
            return ()
        message = MESSAGES.contract_no_boundary.format(config=CONFIG_FILENAME)
        evidence = MESSAGES.contract_no_boundary_evidence
        gap = DriftGap(GAP_NO_BOUNDARY_GLOBS)
        return (_not_checked(CHECK_CONTRACT_DRIFT, "", message, evidence, gap),)
    findings = [found for pair in pairs for found in _drift(pair, globs)]
    return tuple(sorted(findings, key=_order))


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


def _drift(pair: TextPair, globs: Sequence[str]) -> list[PairFinding]:
    language = pair_language(pair.path)
    if language is None or pair.new is None or not contracts.in_boundary(pair.path, globs):
        return []
    if not contracts.rules_for(language):
        message = MESSAGES.contract_no_rules.format(language=language, path=pair.path)
        gap = DriftGap(GAP_NO_RULES, language)
        return [_not_checked(CHECK_CONTRACT_DRIFT, pair.path, message, language, gap)]

    old_units = _units(
        pair.old, pair.path, language, MESSAGES.contract_old_not_measured, CHECK_CONTRACT_DRIFT
    )
    if isinstance(old_units, PairFinding):
        return [old_units]
    new_units = _units(
        pair.new, pair.path, language, MESSAGES.contract_new_not_measured, CHECK_CONTRACT_DRIFT
    )
    if isinstance(new_units, PairFinding):
        return [new_units]

    old_hits = () if pair.old is None else contracts.hits(pair.old, language, old_units)
    new_hits = contracts.hits(pair.new, language, new_units)
    return _new_hits(pair.path, old_hits, new_hits, _identities(old_units, new_units))


def _identities(old_units: Sequence[Unit], new_units: Sequence[Unit]) -> dict[int, str]:
    # A new unit that `_match` pairs keeps its old name, so a rename moves no hit.
    return {
        match.new.start_byte: (match.new if match.old is None else match.old).qualname
        for match in _match(old_units, new_units)
    }


def _new_hits(
    path: str, old_hits: Sequence[Hit], new_hits: Sequence[Hit], identity: dict[int, str]
) -> list[PairFinding]:
    carried = Counter(
        _key(hit, None if hit.unit is None else hit.unit.qualname) for hit in old_hits
    )
    fresh: list[PairFinding] = []
    for hit in new_hits:
        key = _key(hit, None if hit.unit is None else identity[hit.unit.start_byte])
        if carried[key]:
            carried[key] -= 1
        else:
            fresh.append(_hit_finding(path, hit, key))
    return fresh


def _key(hit: Hit, unit: str | None) -> DriftKey:
    return DriftKey(
        rule=hit.rule.id, unit=MODULE_UNIT if unit is None else unit, capture=hit.capture
    )


def _hit_finding(path: str, hit: Hit, key: DriftKey) -> PairFinding:
    fields = {"rule": key.rule, "unit": key.unit, "capture": key.capture}
    finding = Finding(
        check=CHECK_CONTRACT_DRIFT,
        severity=Severity(hit.rule.severity),
        message=MESSAGES.contract_new_hit.format(why=hit.rule.message, **fields),
        path=path,
        line=hit.line,
        evidence=MESSAGES.contract_evidence.format(**fields),
    )
    return PairFinding(finding, key)
