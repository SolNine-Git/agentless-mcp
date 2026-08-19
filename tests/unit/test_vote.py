"""The rerank ladder and the cluster ordering, transcribed from the original.

The ladder is table-driven because that is the shape of the thing being
characterized: rerank.py's ``majority_voting`` is three ``if`` branches over
the same candidate list, and the property worth pinning is *which* branch
answers for a given set of verdicts. Each row is one such set.

The cluster ordering gets its own tests because its tiebreak is the part that
silently stops mattering: with two equal-sized clusters, dropping
``-first_appear_idx`` still produces a plausible ranked list, just a different
one on every run.
"""

import pytest

from agentless_mcp.core import vote
from agentless_mcp.core.vote import (
    TIER_APPLIED,
    TIER_NONE,
    TIER_REGRESSION,
    TIER_REPRODUCTION,
    VoteCandidate,
)

REGRESSION = "regression"
REPRODUCTION = "reproduction"


def candidate(name, index, *, key="k1", passes=(), **flags):
    """Build one vote input; ``passes`` names the suites this candidate passed.

    ``applied`` and ``measured`` are true unless a row says otherwise, and an
    unrecognised flag is refused rather than ignored: a mistyped ``measured``
    that silently stayed true would make the test that needs it pass for the
    wrong reason.
    """
    unknown = sorted(set(flags) - {"applied", "measured"})
    if unknown:
        message = f"unknown candidate flag(s): {unknown}"
        raise TypeError(message)
    return VoteCandidate(
        id=name,
        index=index,
        applied=flags.get("applied", True),
        measured=flags.get("measured", True),
        equivalence_key=key,
        regression_passed=REGRESSION in passes,
        reproduction_passed=REPRODUCTION in passes,
    )


class TestLadder:
    """Which rung answers, for each shape of verdict set."""

    @pytest.mark.parametrize(
        ("label", "candidates", "repro_valid", "expected_tier", "expected_ids"),
        [
            (
                "reproduction and regression both pass: the top rung answers",
                [
                    candidate("a", 0, passes=(REGRESSION, REPRODUCTION)),
                    candidate("b", 1, key="k2", passes=(REGRESSION,)),
                ],
                True,
                TIER_REPRODUCTION,
                ["a"],
            ),
            (
                "nothing fixed the repro: fall through to regression only",
                [
                    candidate("a", 0, passes=(REGRESSION,)),
                    candidate("b", 1, key="k2"),
                ],
                True,
                TIER_REGRESSION,
                ["a"],
            ),
            (
                "nothing passed the regression suite: fall through to applied",
                [
                    candidate("a", 0),
                    candidate("b", 1, key="k2"),
                ],
                True,
                TIER_APPLIED,
                ["a", "b"],
            ),
            (
                "an invalid repro removes the rung rather than failing it",
                [
                    candidate("a", 0, passes=(REGRESSION,)),
                    candidate("b", 1, key="k2", passes=(REGRESSION, REPRODUCTION)),
                ],
                False,
                TIER_REGRESSION,
                ["a", "b"],
            ),
            (
                "no repro command at all behaves the same way",
                [candidate("a", 0, passes=(REGRESSION,))],
                False,
                TIER_REGRESSION,
                ["a"],
            ),
            (
                "a patch that did not apply never votes, at any rung",
                [
                    candidate("a", 0, applied=False, passes=(REGRESSION, REPRODUCTION)),
                    candidate("b", 1, key="k2"),
                ],
                True,
                TIER_APPLIED,
                ["b"],
            ),
            (
                "an applied patch with no equivalence key never votes either",
                [
                    candidate("a", 0, key=None, passes=(REGRESSION,)),
                    candidate("b", 1, key="k2"),
                ],
                True,
                TIER_APPLIED,
                ["b"],
            ),
            (
                "nothing eligible at all is its own tier, not an empty top rung",
                [candidate("a", 0, applied=False)],
                True,
                TIER_NONE,
                [],
            ),
            (
                "a candidate whose test command never ran is not in the applied tier",
                [
                    candidate("a", 0, measured=False),
                    candidate("b", 1, key="k2"),
                ],
                True,
                TIER_APPLIED,
                ["b"],
            ),
        ],
    )
    def test_the_tier_and_its_survivors(
        self, label, candidates, repro_valid, expected_tier, expected_ids
    ):
        report = vote.rank(candidates, repro_valid=repro_valid)
        members = [name for cluster in report.clusters for name in cluster.members]

        assert report.tier == expected_tier, label
        assert sorted(members) == sorted(expected_ids), label

    def test_a_regression_only_survivor_does_not_reach_the_top_rung(self):
        """The empty top rung must fall through, not answer with nothing."""
        report = vote.rank([candidate("a", 0, passes=(REGRESSION,))], repro_valid=True)

        assert report.tier == TIER_REGRESSION
        assert report.survived == 1

    def test_a_run_where_nothing_was_measured_crowns_nobody(self):
        """Three candidates whose test command never started rank as nothing.

        The shape the audit reproduced: every candidate applied, every one of
        them errored before the suite ran, and the ladder answered ``applied``
        with a winner. A tier that says "nothing passed the regression suite"
        is a statement about a suite that ran.
        """
        report = vote.rank(
            [candidate(name, index, measured=False) for index, name in enumerate("abc")],
            repro_valid=False,
        )

        assert report.tier == TIER_NONE
        assert report.winner is None
        assert report.survived == 0
        assert report.clusters == ()
        assert all("nothing was measured" in reason for _, reason in report.excluded)

    def test_the_exclusion_reason_is_reported(self):
        report = vote.rank(
            [candidate("a", 0, applied=False), candidate("b", 1, key=None)],
            repro_valid=False,
        )

        assert dict(report.excluded)["a"] == "the patch did not apply"
        assert "changed nothing" in dict(report.excluded)["b"]
        assert report.considered == 2
        assert report.survived == 0


class TestClusterOrdering:
    def test_the_larger_cluster_ranks_first(self):
        report = vote.rank(
            [
                candidate("a", 0, key="lonely", passes=(REGRESSION,)),
                candidate("b", 1, key="popular", passes=(REGRESSION,)),
                candidate("c", 2, key="popular", passes=(REGRESSION,)),
            ],
            repro_valid=False,
        )

        assert [cluster.members for cluster in report.clusters] == [("b", "c"), ("a",)]
        assert report.clusters[0].rank == 1
        assert report.winner == "b"

    def test_equal_sized_clusters_break_on_first_appearance(self):
        report = vote.rank(
            [
                candidate("late", 3, key="second", passes=(REGRESSION,)),
                candidate("early", 0, key="first", passes=(REGRESSION,)),
                candidate("also-late", 4, key="second", passes=(REGRESSION,)),
                candidate("also-early", 1, key="first", passes=(REGRESSION,)),
            ],
            repro_valid=False,
        )

        assert [cluster.key for cluster in report.clusters] == ["first", "second"]
        assert report.clusters[0].first_appearance == 0
        assert report.clusters[1].first_appearance == 3

    def test_the_representative_is_the_first_appearing_member(self):
        report = vote.rank(
            [
                candidate("second", 5, key="k", passes=(REGRESSION,)),
                candidate("first", 2, key="k", passes=(REGRESSION,)),
            ],
            repro_valid=False,
        )

        assert report.clusters[0].representative == "first"
        assert report.clusters[0].members == ("first", "second")

    def test_a_single_candidate_is_a_cluster_of_one(self):
        report = vote.rank([candidate("only", 0, passes=(REGRESSION,))], repro_valid=False)

        assert report.clusters[0].size == 1
        assert report.clusters[0].members == ("only",)
        assert report.winner == "only"

    def test_input_order_does_not_change_the_ranking(self):
        candidates = [
            candidate("a", 0, key="k1", passes=(REGRESSION,)),
            candidate("b", 1, key="k1", passes=(REGRESSION,)),
            candidate("c", 2, key="k2", passes=(REGRESSION,)),
        ]
        forward = vote.rank(candidates, repro_valid=False)
        backward = vote.rank(list(reversed(candidates)), repro_valid=False)

        assert forward.as_dict() == backward.as_dict()

    def test_the_cluster_counts_its_own_verdicts(self):
        report = vote.rank(
            [
                candidate("a", 0, key="k", passes=(REGRESSION, REPRODUCTION)),
                candidate("b", 1, key="k", passes=(REGRESSION, REPRODUCTION)),
            ],
            repro_valid=True,
        )

        assert report.clusters[0].regression_passed == 2
        assert report.clusters[0].reproduction_passed == 2


class TestRendering:
    def test_the_text_report_names_the_tier_and_the_members(self):
        report = vote.rank(
            [
                candidate("fix-a", 0, key="shared", passes=(REGRESSION, REPRODUCTION)),
                candidate("fix-b", 1, key="shared", passes=(REGRESSION, REPRODUCTION)),
                candidate("fix-c", 2, key="other", passes=(REGRESSION, REPRODUCTION)),
                candidate("broken", 3, applied=False),
            ],
            repro_valid=True,
        )
        text = report.text()

        assert TIER_REPRODUCTION in text
        assert "rank 1  2 candidates" in text
        assert "representative: fix-a (first appearance #0)" in text
        assert "members: fix-a, fix-b" in text
        assert "broken -- the patch did not apply" in text

    def test_an_empty_ranking_says_so(self):
        report = vote.rank([candidate("a", 0, applied=False)], repro_valid=False)

        assert "no candidate survived the ladder" in report.text()
        assert report.winner is None

    def test_the_json_form_carries_the_same_answer(self):
        report = vote.rank([candidate("a", 0, passes=(REGRESSION,))], repro_valid=False)
        document = report.as_dict()

        assert document["tier"] == TIER_REGRESSION
        assert document["winner"] == "a"
        assert document["clusters"][0]["members"] == ["a"]
        assert document["considered"] == 1

    def test_the_summary_line_reports_the_tier(self):
        report = vote.rank([candidate("a", 0, passes=(REGRESSION,))], repro_valid=False)

        assert TIER_REGRESSION in report.summary_line()
        assert "winner a" in report.summary_line()
