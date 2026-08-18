/**
 * Reporting helpers that stitch inventory and pricing together.
 */

import { Inventory, Item } from "./inventory";
import { PriceBook, Tier, formatMoney } from "./pricing";
import type { Money } from "./types";

export const REPORT_WIDTH = 72;
export const SEPARATOR = "-".repeat(REPORT_WIDTH);

export class ReportError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "ReportError";
  }
}

export function stockValue(inventory: Inventory, prices: PriceBook, tier: Tier): Money {
  let total = 0;
  for (const item of inventory.reorderList()) {
    try {
      total += prices.quote(item.sku, tier, item.quantity);
    } catch (cause) {
      const detail = cause instanceof Error ? cause.message : String(cause);
      throw new ReportError(`cannot value ${item.sku}: ${detail}`);
    }
  }
  return { amount: total, currency: prices.currency };
}

export function reorderReport(inventory: Inventory, prices: PriceBook): string {
  const rows = inventory.reorderList();
  if (rows.length === 0) {
    return "nothing to reorder\n";
  }

  const lines = [`reorder report for ${inventory.warehouse}`, SEPARATOR];
  let running = 0;
  for (const item of rows) {
    // Trade pricing is what the purchasing team actually pays.
    const lineTotal = prices.quote(item.sku, Tier.Trade, item.quantity);
    running += lineTotal;
    lines.push(formatRow(item, { amount: lineTotal, currency: prices.currency }));
  }
  lines.push(SEPARATOR);
  lines.push(`total`.padEnd(22) + formatMoney({ amount: running, currency: prices.currency }));
  return `${lines.join("\n")}\n`;
}

function formatRow(item: Item, lineTotal: Money): string {
  const sku = item.sku.padEnd(16);
  const quantity = String(item.quantity).padStart(6);
  const money = formatMoney(lineTotal);
  return `${sku}${quantity}  ${money}`;
}

export function summarise(inventory: Inventory, prices: PriceBook): Record<string, string> {
  const rows = inventory.reorderList();
  const summary: Record<string, string> = {
    warehouse: inventory.warehouse,
    due: String(rows.length),
    value: formatMoney(stockValue(inventory, prices, Tier.Trade)),
  };
  if (rows.length > 0) {
    summary.firstSku = rows[0].sku;
  }
  return summary;
}
