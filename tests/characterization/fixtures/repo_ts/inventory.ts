/**
 * Inventory tracking for a small warehouse.
 */

import { PriceBook, Tier } from "./pricing";
import type { Money } from "./types";

export const MAX_ITEMS = 500;
export const DEFAULT_WAREHOUSE = "main";

// Reorder thresholds are per-category because bolts and lathes do not move
// at remotely the same rate.
export const REORDER_THRESHOLDS: Record<string, number> = {
  fastener: 250,
  tool: 5,
  consumable: 40,
};

export interface Item {
  sku: string;
  category: string;
  quantity: number;
}

export function needsReorder(item: Item): boolean {
  const threshold = REORDER_THRESHOLDS[item.category];
  if (threshold === undefined) {
    // An unknown category is a data error, not a reason to reorder.
    return false;
  }
  if (item.quantity < 0) {
    throw new Error(`${item.sku} has a negative quantity`);
  }
  return item.quantity < threshold;
}

export class Inventory {
  private readonly items: Map<string, Item>;

  private readonly history: string[];

  constructor(public readonly warehouse: string = DEFAULT_WAREHOUSE) {
    if (warehouse.length === 0) {
      throw new Error("warehouse name must not be empty");
    }
    this.items = new Map();
    this.history = [];
  }

  add(item: Item): void {
    if (this.items.size >= MAX_ITEMS) {
      throw new Error(`${this.warehouse} already holds ${MAX_ITEMS} items`);
    }
    const existing = this.items.get(item.sku);
    if (existing !== undefined) {
      this.items.set(item.sku, {
        sku: item.sku,
        category: item.category,
        quantity: existing.quantity + item.quantity,
      });
      this.history.push(`merge ${item.sku}`);
      return;
    }
    this.items.set(item.sku, item);
    this.history.push(`add ${item.sku}`);
  }

  remove(sku: string): Item {
    const item = this.items.get(sku);
    if (item === undefined) {
      // A missing SKU is a caller bug, not an empty result.
      throw new Error(`${sku} is not stocked in ${this.warehouse}`);
    }
    this.items.delete(sku);
    this.history.push(`remove ${sku}`);
    return item;
  }

  reorderList(): Item[] {
    const due: Item[] = [];
    for (const item of this.items.values()) {
      if (needsReorder(item)) {
        due.push(item);
      }
    }
    due.sort((left, right) => left.sku.localeCompare(right.sku));
    return due;
  }

  valueAt(prices: PriceBook, tier: Tier): Money {
    let total = 0;
    for (const item of this.reorderList()) {
      total += prices.quote(item.sku, tier, item.quantity);
    }
    return { amount: total, currency: prices.currency };
  }

  auditTrail(): string[] {
    return this.history.map((entry, index) => {
      const position = String(index + 1).padStart(3, "0");
      return `${position} ${entry}`;
    });
  }
}
