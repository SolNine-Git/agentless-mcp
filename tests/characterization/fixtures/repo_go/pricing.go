// Package warehouse: price calculation with tiered discounts.
package warehouse

import (
	"fmt"
	"math"
	"sort"
)

// BaseMarkup is applied to every cost price before discounts.
const BaseMarkup = 1.35

// Currency is the only currency this book quotes in.
const Currency = "EUR"

// Tier names a customer pricing tier.
type Tier string

// The tiers the business sells at.
const (
	TierRetail    Tier = "retail"
	TierTrade     Tier = "trade"
	TierWholesale Tier = "wholesale"
)

var discounts = map[Tier]float64{
	TierRetail:    0.0,
	TierTrade:     0.10,
	TierWholesale: 0.22,
}

// PriceBook holds cost prices keyed by SKU.
type PriceBook struct {
	costs map[string]float64
}

// NewPriceBook copies the cost table so callers cannot mutate it later.
func NewPriceBook(costs map[string]float64) (*PriceBook, error) {
	copied := make(map[string]float64, len(costs))
	for sku, cost := range costs {
		if cost < 0 {
			return nil, fmt.Errorf("negative cost price for %q", sku)
		}
		copied[sku] = cost
	}
	return &PriceBook{costs: copied}, nil
}

// CostOf returns the cost price for a SKU.
func (p *PriceBook) CostOf(sku string) (float64, error) {
	cost, ok := p.costs[sku]
	if !ok {
		return 0, fmt.Errorf("no cost price recorded for %q", sku)
	}
	return cost, nil
}

// Quote prices a line at a tier.
func (p *PriceBook) Quote(sku string, tier Tier, quantity int) (float64, error) {
	if quantity < 1 {
		return 0, fmt.Errorf("quantity must be at least one, got %d", quantity)
	}
	cost, err := p.CostOf(sku)
	if err != nil {
		return 0, err
	}
	// Discounts apply to the marked-up price, not the cost price.
	unit := cost * BaseMarkup * (1 - discounts[tier])
	return math.Round(unit*float64(quantity)*100) / 100, nil
}

// PriceList returns every SKU with its unit price at a tier, in SKU order.
func (p *PriceBook) PriceList(tier Tier) ([][2]interface{}, error) {
	skus := make([]string, 0, len(p.costs))
	for sku := range p.costs {
		skus = append(skus, sku)
	}
	sort.Strings(skus)

	rows := make([][2]interface{}, 0, len(skus))
	for _, sku := range skus {
		price, err := p.Quote(sku, tier, 1)
		if err != nil {
			return nil, err
		}
		rows = append(rows, [2]interface{}{sku, price})
	}
	return rows, nil
}

// FormatMoney renders an amount with the currency suffix.
func FormatMoney(amount float64) string {
	return fmt.Sprintf("%.2f %s", amount, Currency)
}
