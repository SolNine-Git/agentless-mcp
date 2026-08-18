// Package warehouse: reporting helpers over inventory and pricing.
package warehouse

import (
	"fmt"
	"strings"
)

// ReportWidth is the fixed width of a rendered report.
const ReportWidth = 72

// Separator rules off the report header and footer.
var Separator = strings.Repeat("-", ReportWidth)

// ReportError reports that a report could not be produced from the data.
type ReportError struct {
	Missing []string
}

// Error implements the error interface.
func (e *ReportError) Error() string {
	return fmt.Sprintf(
		"no cost price for %d SKUs: %s",
		len(e.Missing),
		strings.Join(e.Missing, ", "),
	)
}

// StockValue totals the quoted value of everything due for reorder.
func StockValue(inv *Inventory, prices *PriceBook, tier Tier) (float64, error) {
	rows, err := inv.ReorderList()
	if err != nil {
		return 0, err
	}
	total := 0.0
	for _, item := range rows {
		line, err := prices.Quote(item.SKU, tier, item.Quantity)
		if err != nil {
			return 0, fmt.Errorf("cannot value %s: %w", item.SKU, err)
		}
		total += line
	}
	return total, nil
}

// ReorderReport renders a plain-text reorder report.
func ReorderReport(inv *Inventory, prices *PriceBook) (string, error) {
	rows, err := inv.ReorderList()
	if err != nil {
		return "", err
	}
	if len(rows) == 0 {
		return "nothing to reorder\n", nil
	}

	lines := []string{fmt.Sprintf("reorder report for %s", inv.Warehouse), Separator}
	running := 0.0
	for _, item := range rows {
		// Trade pricing is what the purchasing team actually pays.
		lineTotal, err := prices.Quote(item.SKU, TierTrade, item.Quantity)
		if err != nil {
			return "", err
		}
		running += lineTotal
		lines = append(lines, formatRow(item, lineTotal))
	}
	lines = append(lines, Separator)
	lines = append(lines, fmt.Sprintf("%-22s%s", "total", FormatMoney(running)))
	return strings.Join(lines, "\n") + "\n", nil
}

func formatRow(item Item, lineTotal float64) string {
	sku := fmt.Sprintf("%-16s", item.SKU)
	quantity := fmt.Sprintf("%6d", item.Quantity)
	return sku + quantity + "  " + FormatMoney(lineTotal)
}

// CheckReportInputs fails loudly when a report would silently under-report.
func CheckReportInputs(inv *Inventory, prices *PriceBook) error {
	rows, err := inv.ReorderList()
	if err != nil {
		return err
	}
	var missing []string
	for _, item := range rows {
		if _, err := prices.CostOf(item.SKU); err != nil {
			missing = append(missing, item.SKU)
		}
	}
	if len(missing) > 0 {
		return &ReportError{Missing: missing}
	}
	return nil
}
