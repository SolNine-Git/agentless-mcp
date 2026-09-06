"""Max-min fair budget splitting, shared by every view that must cut what it shows.

The allocation is the classic water-filling one, and it is what makes the
degradation fair rather than positional. Every round divides what is left of
the budget equally among the items still competing; the items that already fit
their share are settled at full length and give their unspent tokens back; the
rest go round again on a larger share. The loop ends when a round settles
nobody, and every item still competing then gets exactly the same allowance --
so a thousand-line class and a five-line method are cut to the same size, and
no item is cut at all while another is still whole and larger.

One home for the rule, because two copies of it drift: ``expand`` cuts symbol
cards and ``history`` cuts commit bodies, and a fairness fix applied to one of
them but not the other is a difference no test would name. What each caller
keeps is what only it can know -- how an item costs and how an item shortens.

``TRUNCATION_MARKER_TOKENS`` is the room kept back on each shortened item for
the marker that says it was shortened, so announcing the cut cannot itself be
what pushes an item past its share.
"""

from collections.abc import Sequence

TRUNCATION_MARKER_TOKENS = 32


def allocate(costs: Sequence[int], budget: int) -> tuple[frozenset[int], int]:
    """Split ``budget`` across ``costs`` max-min fair; return the indices to cut and their share."""
    pending = set(range(len(costs)))
    remaining = budget

    while pending:
        share = remaining // len(pending)
        settled = {index for index in pending if costs[index] <= share}
        if not settled:
            break
        remaining -= sum(costs[index] for index in settled)
        pending -= settled

    if not pending:
        return frozenset(), 0
    return frozenset(pending), max(0, remaining // len(pending))
