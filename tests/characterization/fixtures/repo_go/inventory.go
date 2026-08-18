// Package warehouse tracks stock for a small warehouse.
package warehouse

import (
	"fmt"
	"sort"
)

// MaxItems caps how much one warehouse may hold.
const MaxItems = 500

// DefaultWarehouse is used when no warehouse is named.
const DefaultWarehouse = "main"

// ReorderThresholds are per-category because bolts and lathes do not move
// at remotely the same rate.
var ReorderThresholds = map[string]int{
	"fastener":   250,
	"tool":       5,
	"consumable": 40,
}

// Item is a single stocked item.
type Item struct {
	SKU      string
	Category string
	Quantity int
}

// NeedsReorder reports whether the item fell under its category threshold.
func (i Item) NeedsReorder() (bool, error) {
	threshold, ok := ReorderThresholds[i.Category]
	if !ok {
		// An unknown category is a data error, not a reason to reorder.
		return false, nil
	}
	if i.Quantity < 0 {
		return false, fmt.Errorf("%s has a negative quantity", i.SKU)
	}
	return i.Quantity < threshold, nil
}

// Inventory is the stock held in one warehouse.
type Inventory struct {
	Warehouse string
	items     map[string]Item
	history   []string
}

// NewInventory starts an empty inventory for a warehouse.
func NewInventory(warehouse string) *Inventory {
	if warehouse == "" {
		warehouse = DefaultWarehouse
	}
	return &Inventory{
		Warehouse: warehouse,
		items:     map[string]Item{},
		history:   nil,
	}
}

// Add stores an item, refusing to exceed the per-warehouse ceiling.
func (inv *Inventory) Add(item Item) error {
	if len(inv.items) >= MaxItems {
		return fmt.Errorf("%s already holds %d items", inv.Warehouse, MaxItems)
	}
	if existing, ok := inv.items[item.SKU]; ok {
		merged := Item{
			SKU:      item.SKU,
			Category: item.Category,
			Quantity: existing.Quantity + item.Quantity,
		}
		inv.items[item.SKU] = merged
		inv.history = append(inv.history, "merge "+item.SKU)
		return nil
	}
	inv.items[item.SKU] = item
	inv.history = append(inv.history, "add "+item.SKU)
	return nil
}

// Remove deletes an item by SKU and returns it.
func (inv *Inventory) Remove(sku string) (Item, error) {
	item, ok := inv.items[sku]
	if !ok {
		return Item{}, fmt.Errorf("%q is not stocked in %s", sku, inv.Warehouse)
	}
	delete(inv.items, sku)
	inv.history = append(inv.history, "remove "+sku)
	return item, nil
}

// ReorderList returns every item that fell under its threshold.
func (inv *Inventory) ReorderList() ([]Item, error) {
	var out []Item
	for _, item := range inv.items {
		due, err := item.NeedsReorder()
		if err != nil {
			return nil, err
		}
		if due {
			out = append(out, item)
		}
	}
	sort.Slice(out, func(a, b int) bool { return out[a].SKU < out[b].SKU })
	return out, nil
}

// AuditTrail renders the mutation history oldest first.
func (inv *Inventory) AuditTrail() []string {
	rendered := make([]string, 0, len(inv.history))
	for index, entry := range inv.history {
		rendered = append(rendered, fmt.Sprintf("%03d %s", index+1, entry))
	}
	return rendered
}
