/**
 * Price calculation with tiered discounts.
 */

import type { Money } from "./types";

export const BASE_MARKUP = 1.35;
export const CURRENCY = "EUR";

export enum Tier {
  Retail = "retail",
  Trade = "trade",
  Wholesale = "wholesale",
}

const DISCOUNTS: Record<Tier, number> = {
  [Tier.Retail]: 0,
  [Tier.Trade]: 0.1,
  [Tier.Wholesale]: 0.22,
};

export class PriceBook {
  public readonly currency: string = CURRENCY;

  private readonly costs: Map<string, number>;

  constructor(costs: Record<string, number>) {
    // Copy the table so a caller cannot mutate prices behind our back.
    this.costs = new Map();
    for (const [sku, cost] of Object.entries(costs)) {
      if (cost < 0) {
        throw new Error(`negative cost price for ${sku}`);
      }
      this.costs.set(sku, cost);
    }
  }

  costOf(sku: string): number {
    const cost = this.costs.get(sku);
    if (cost === undefined) {
      throw new Error(`no cost price recorded for ${sku}`);
    }
    return cost;
  }

  quote(sku: string, tier: Tier, quantity = 1): number {
    if (quantity < 1) {
      throw new Error("quantity must be at least one");
    }
    const unit = this.costOf(sku) * BASE_MARKUP;
    const discounted = unit * (1 - DISCOUNTS[tier]);
    const lineTotal = discounted * quantity;
    return Math.round(lineTotal * 100) / 100;
  }

  priceList(tier: Tier): Array<[string, number]> {
    const rows: Array<[string, number]> = [];
    for (const sku of [...this.costs.keys()].sort()) {
      rows.push([sku, this.quote(sku, tier)]);
    }
    return rows;
  }

  applyPromotion(sku: string, tier: Tier, promotion: number): number {
    if (promotion < 0 || promotion > 0.5) {
      throw new RangeError(`promotion out of range for ${sku}: ${promotion}`);
    }
    const base = this.quote(sku, tier, 1);
    const promoted = base * (1 - promotion);
    const floorPrice = this.costOf(sku);
    if (promoted < floorPrice) {
      // Never promote below cost: that is a pricing bug, not a deal.
      throw new RangeError(`promotion on ${sku} would sell below cost`);
    }
    return Math.round(promoted * 100) / 100;
  }

  cheapest(tier: Tier): string {
    const rows = this.priceList(tier);
    if (rows.length === 0) {
      throw new Error("price book is empty");
    }
    let [bestSku, bestPrice] = rows[0];
    for (const [sku, price] of rows.slice(1)) {
      if (price < bestPrice) {
        bestSku = sku;
        bestPrice = price;
      }
    }
    return bestSku;
  }
}

export function formatMoney(money: Money): string {
  return `${money.amount.toFixed(2)} ${money.currency}`;
}
