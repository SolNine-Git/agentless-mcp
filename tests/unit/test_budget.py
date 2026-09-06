"""The max-min fair allocator both expand and history spend their budget with."""

import pytest

from agentless_mcp.util.budget import TRUNCATION_MARKER_TOKENS, allocate


class TestAllocate:
    def test_nothing_to_spend_on_cuts_nothing(self):
        assert allocate([], 1000) == (frozenset(), 0)

    def test_every_item_inside_its_share_is_kept_whole(self):
        assert allocate([10, 20, 30], 1000) == (frozenset(), 0)

    def test_only_the_items_over_their_share_are_cut(self):
        cut, share = allocate([10, 10, 500], 120)

        assert cut == frozenset({2})
        assert share == 100

    def test_a_settled_item_hands_its_leftover_to_the_rest(self):
        # 1 spent of 100 leaves 99 for the two that did not fit, not 33 each.
        cut, share = allocate([1, 400, 400], 100)

        assert cut == frozenset({1, 2})
        assert share == 49

    def test_items_that_all_miss_are_cut_to_the_same_share(self):
        cut, share = allocate([500, 500, 500, 500], 400)

        assert cut == frozenset({0, 1, 2, 3})
        assert share == 100

    @pytest.mark.parametrize("budget", [0, 1])
    def test_a_budget_that_buys_nothing_still_returns_a_share(self, budget):
        cut, share = allocate([500, 500], budget)

        assert cut == frozenset({0, 1})
        assert share >= 0

    def test_the_marker_allowance_is_the_one_both_services_reserve(self):
        assert TRUNCATION_MARKER_TOKENS == 32
