package sample

import "fmt"

func ScaledSum(left int, right int) int {
	total := left + right
	fmt.Println("sum", total)
	return total * 2
}

func DoubledTotal(a int, b int) int {
	sum := a + b
	fmt.Println("total", sum)
	return sum * 3
}

func NearMiss(left int, right int) int {
	total := left - right
	fmt.Println("sum", total)
	return total * 2
}
