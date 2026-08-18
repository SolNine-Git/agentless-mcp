/**
 * Shared value types and the guards that police them.
 */

export interface Money {
  amount: number;
  currency: string;
}

export interface Warehouse {
  name: string;
  region: string;
  active: boolean;
}

// Regions the business actually ships to; anything else is a data error.
export const REGIONS = ["eu-west", "eu-central", "uk"] as const;

export type Region = (typeof REGIONS)[number];

export function isRegion(value: string): value is Region {
  return (REGIONS as readonly string[]).includes(value);
}

export function describeWarehouse(warehouse: Warehouse): string {
  const state = warehouse.active ? "active" : "closed";
  const region = isRegion(warehouse.region) ? warehouse.region : "unknown region";
  return `${warehouse.name} (${region}, ${state})`;
}

export function parseWarehouse(payload: unknown): Warehouse {
  if (typeof payload !== "object" || payload === null) {
    throw new TypeError("warehouse payload must be an object");
  }
  const record = payload as Record<string, unknown>;
  const name = record.name;
  const region = record.region;
  const active = record.active;
  if (typeof name !== "string" || name.length === 0) {
    throw new TypeError("warehouse name must be a non-empty string");
  }
  if (typeof region !== "string" || !isRegion(region)) {
    throw new TypeError(`unknown warehouse region: ${String(region)}`);
  }
  if (typeof active !== "boolean") {
    throw new TypeError("warehouse active flag must be a boolean");
  }
  return { name, region, active };
}

export function addMoney(left: Money, right: Money): Money {
  if (left.currency !== right.currency) {
    throw new TypeError(`cannot add ${left.currency} to ${right.currency}`);
  }
  return { amount: left.amount + right.amount, currency: left.currency };
}
