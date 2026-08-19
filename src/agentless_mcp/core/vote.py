"""The rerank ladder and cluster ordering, re-keyed on AST equivalence.

Ported from Agentless ``agentless/repair/rerank.py`` (``majority_voting``,
L156-289). Two mechanisms come across, and both are the reason the pipeline
worked at all:

**A filter ladder, not a filter.** Candidates are narrowed to the strongest
evidence tier that is not empty -- reproduction *and* regression, else
regression alone, else everything that applied -- and the tier that answered
is reported. Falling through is normal: it says "nothing fixed the bug, here
is the best of what did not break anything", which is a different answer from
"here is the fix" and must not read like one.

**Vote on equivalence classes, not on text.** The original counted identical
``ast.unparse`` output; this counts identical
:mod:`agentless_mcp.core.normalize` keys, which is the same idea done in a
language-agnostic way. Clusters are ordered by size, ties broken by first
appearance -- the ``(vote[key], -first_appear_idx[key])`` tuple at
rerank.py:269, with the sign inverted because ordering here is ascending on
the tiebreak rather than descending on its negation. Sampling twice as much
must not change which of two equal clusters wins, and without the tiebreak it
would.

Two deliberate departures from the original, both narrowing it:

* **Regression is absolute, not relative.** The original took the *minimum*
  number of failing tests across the samples and called everything at that
  minimum a pass, so a run where every candidate broke four tests had a full
  tier of "passing" candidates. Here a regression verdict is pass or fail, and
  a run where nothing passes falls through to the apply-ok tier and says so.
  A relative floor is a way of never reporting an empty tier, which is exactly
  what the caller needs to be told.
* **An unusable key is exclusion, everywhere.** The original required a
  non-empty ``normalized_patch`` in all three branches, and so does this: a
  candidate that did not apply, or that applied and changed nothing, has no
  equivalence class to vote in and is listed as excluded rather than counted.

**A candidate nobody measured does not rank.** ``measured`` is the caller's
statement that the regression command actually ran for this candidate. When it
did not -- it could not be spawned, or the run never reached the candidate at
all -- the candidate is excluded rather than ranked in the fall-through tier.
"Applied cleanly, nothing passed the regression suite" is a claim about a
suite that ran; saying it of a run that never happened is inventing evidence,
and a crowned winner out of a run where nothing was measured is the worst
shape that mistake can take.

Pure ranking over already-computed verdicts. Nothing here runs a test, reads a
file or knows what a repository is.
"""

from collections.abc import Sequence
from dataclasses import dataclass
from typing import Any

# The ladder's rungs, strongest evidence first. These strings are part of the
# report: an agent reading a ranked list needs to know which question the
# ranking answered.
TIER_REPRODUCTION = "regression+reproduction"
TIER_REGRESSION = "regression"
TIER_APPLIED = "applied"
TIER_NONE = "none"

_TIER_DETAIL = {
    TIER_REPRODUCTION: "fixed the reproduction test and broke no regression test",
    TIER_REGRESSION: "broke no regression test (nothing fixed the reproduction test)",
    TIER_APPLIED: "applied cleanly (nothing passed the regression suite)",
    TIER_NONE: "no candidate applied cleanly with a usable equivalence key",
}


@dataclass(frozen=True)
class VoteCandidate:
    """One validated candidate, reduced to what the ladder and the vote need.

    ``index`` is the candidate's position in first-appearance order, which is
    the sorted order of the candidates directory. It is the tiebreak between
    equal-sized clusters and therefore has to be stable across runs, which is
    why it is carried rather than recomputed from whatever order the verdicts
    happened to be read in.

    ``measured`` and ``regression_passed`` answer different questions, and
    collapsing them is how "the test command could not be started" turns into
    "the patch broke the tests". It has no default: a caller who does not know
    whether anything ran has to say so.
    """

    id: str
    index: int
    applied: bool
    measured: bool
    equivalence_key: str | None
    regression_passed: bool
    reproduction_passed: bool


@dataclass(frozen=True)
class Cluster:
    """One equivalence class of candidates, and how it did.

    ``representative`` is the first-appearing member: with every member making
    the same structural change, any of them would do, and picking the earliest
    makes the choice reproducible.
    """

    rank: int
    key: str
    size: int
    members: tuple[str, ...]
    representative: str
    first_appearance: int
    regression_passed: int
    reproduction_passed: int

    def as_dict(self) -> dict[str, Any]:
        """Return the JSON form of this cluster."""
        return {
            "rank": self.rank,
            "key": self.key,
            "size": self.size,
            "members": list(self.members),
            "representative": self.representative,
            "first_appearance": self.first_appearance,
            "regression_passed": self.regression_passed,
            "reproduction_passed": self.reproduction_passed,
        }


@dataclass(frozen=True)
class VoteReport:
    """A ranked answer: which tier decided it, and the clusters in order."""

    tier: str
    tier_detail: str
    repro_valid: bool
    considered: int
    survived: int
    clusters: tuple[Cluster, ...]
    excluded: tuple[tuple[str, str], ...]

    @property
    def winner(self) -> str | None:
        """The representative of the top cluster, or None when nothing ranked."""
        return self.clusters[0].representative if self.clusters else None

    def as_dict(self) -> dict[str, Any]:
        """Return the JSON form of this report."""
        return {
            "tier": self.tier,
            "tier_detail": self.tier_detail,
            "repro_valid": self.repro_valid,
            "considered": self.considered,
            "survived": self.survived,
            "winner": self.winner,
            "clusters": [cluster.as_dict() for cluster in self.clusters],
            "excluded": [{"id": name, "reason": reason} for name, reason in self.excluded],
        }

    def summary_line(self) -> str:
        """Return the one-line summary for the receipt on stderr."""
        count = len(self.clusters)
        return (
            f"tier '{self.tier}': {self.survived} of {self.considered} candidates in "
            f"{count} cluster{'' if count == 1 else 's'}; winner {self.winner or 'none'}"
        )

    def text(self) -> str:
        """Render the report as incident cards, names not opaque ids."""
        return render(self)


def rank(candidates: Sequence[VoteCandidate], *, repro_valid: bool) -> VoteReport:
    """Apply the filter ladder and rank the surviving equivalence clusters.

    ``repro_valid`` is the caller's statement that the reproduction test
    actually reproduces the bug -- that it failed on the unpatched baseline.
    When it is false the reproduction rung is removed from the ladder rather
    than failed: a test that passes before the fix says nothing about the fix,
    and ranking on it would promote whichever candidate happened to leave it
    passing.
    """
    eligible: list[VoteCandidate] = []
    excluded: list[tuple[str, str]] = []
    for candidate in sorted(candidates, key=lambda entry: entry.index):
        reason = _exclusion(candidate)
        if reason is None:
            eligible.append(candidate)
        else:
            excluded.append((candidate.id, reason))

    tier, survivors = _ladder(eligible, repro_valid=repro_valid)
    return VoteReport(
        tier=tier,
        tier_detail=_TIER_DETAIL[tier],
        repro_valid=repro_valid,
        considered=len(candidates),
        survived=len(survivors),
        clusters=_clusters(survivors),
        excluded=tuple(excluded),
    )


def render(report: VoteReport) -> str:
    """Render one report as text: the header, then a card per cluster."""
    lines = [
        f"# vote over {report.considered} candidates -- tier '{report.tier}'",
        f"# {report.tier_detail}",
        "# reproduction test "
        + (
            "reproduced the bug on the baseline and counted"
            if report.repro_valid
            else "was not usable and was excluded from the ladder"
        ),
        "",
    ]

    if not report.clusters:
        lines.append("no candidate survived the ladder; there is nothing to rank.")
    for cluster in report.clusters:
        lines.extend(_card(cluster))

    if report.excluded:
        lines.append("excluded before the ladder:")
        lines.extend(f"  {name} -- {reason}" for name, reason in report.excluded)
        lines.append("")

    return "\n".join(lines).rstrip("\n") + "\n"


def _card(cluster: Cluster) -> list[str]:
    """Render one cluster as an incident card."""
    plural = "candidate" if cluster.size == 1 else "candidates"
    return [
        f"rank {cluster.rank}  {cluster.size} {plural}  key {cluster.key[:16]}",
        (
            f"  representative: {cluster.representative}"
            f" (first appearance #{cluster.first_appearance})"
        ),
        f"  members: {', '.join(cluster.members)}",
        (
            f"  regression passed {cluster.regression_passed}/{cluster.size}"
            f"   reproduction passed {cluster.reproduction_passed}/{cluster.size}"
        ),
        "",
    ]


def _exclusion(candidate: VoteCandidate) -> str | None:
    """Return why a candidate cannot vote at all, or None when it can."""
    if not candidate.applied:
        return "the patch did not apply"
    if not candidate.measured:
        return "the test command never ran for it, so nothing was measured"
    if not candidate.equivalence_key:
        return "the patch applied but changed nothing, so it has no equivalence key"
    return None


def _ladder(
    eligible: Sequence[VoteCandidate], *, repro_valid: bool
) -> tuple[str, list[VoteCandidate]]:
    """Return the strongest non-empty tier and the candidates in it."""
    rungs: list[tuple[str, list[VoteCandidate]]] = []
    if repro_valid:
        rungs.append(
            (
                TIER_REPRODUCTION,
                [c for c in eligible if c.regression_passed and c.reproduction_passed],
            )
        )
    rungs.append((TIER_REGRESSION, [c for c in eligible if c.regression_passed]))
    rungs.append((TIER_APPLIED, list(eligible)))

    for tier, survivors in rungs:
        if survivors:
            return tier, survivors
    return TIER_NONE, []


def _clusters(survivors: Sequence[VoteCandidate]) -> tuple[Cluster, ...]:
    """Group survivors by equivalence key and order the groups.

    Size descending, first appearance ascending. Both halves matter: size is
    the vote, and first appearance is what keeps the answer from depending on
    dictionary order the moment two clusters are the same size.
    """
    grouped: dict[str, list[VoteCandidate]] = {}
    for candidate in survivors:
        # Eligibility already established a key; the check is for the type.
        key = candidate.equivalence_key or ""
        grouped.setdefault(key, []).append(candidate)

    ordered = sorted(
        grouped.items(),
        key=lambda item: (-len(item[1]), min(entry.index for entry in item[1])),
    )

    clusters: list[Cluster] = []
    for position, (key, members) in enumerate(ordered, start=1):
        in_order = sorted(members, key=lambda entry: entry.index)
        clusters.append(
            Cluster(
                rank=position,
                key=key,
                size=len(in_order),
                members=tuple(entry.id for entry in in_order),
                representative=in_order[0].id,
                first_appearance=in_order[0].index,
                regression_passed=sum(1 for entry in in_order if entry.regression_passed),
                reproduction_passed=sum(1 for entry in in_order if entry.reproduction_passed),
            )
        )
    return tuple(clusters)
