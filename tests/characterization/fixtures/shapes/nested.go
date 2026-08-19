package shapes

import "sort"

// Sorted returns the values in ascending order.
func Sorted(values []int) []int {
	out := append([]int(nil), values...)
	sort.Slice(out, func(a, b int) bool { return out[a] < out[b] })
	return out
}
