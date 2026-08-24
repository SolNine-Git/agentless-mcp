"""Customers and the pricing tier each one buys at."""

from dataclasses import dataclass

from pricing import Tier

DEFAULT_TIER = Tier.RETAIL


@dataclass(frozen=True)
class Customer:
    """One account, with the tier its quotes are priced at."""

    account: str
    tier: Tier = DEFAULT_TIER

    def is_trade(self) -> bool:
        """True when the account buys at a discounted tier."""
        return self.tier is not Tier.RETAIL


def tier_of(customer: Customer) -> Tier:
    """The tier to quote this customer at."""
    return customer.tier
