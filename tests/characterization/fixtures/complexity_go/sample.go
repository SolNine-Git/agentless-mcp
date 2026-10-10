package sample

type Store struct{ items []int }

func (s *Store) Find(key int) int {
	// A comment line does not count.

	label := `one
two
three`
	_ = label
	for i, v := range s.items {
		if v == key && i > 0 || key < 0 {
			return i
		}
	}
	switch key {
	case 1, 2:
		return 1
	case 3:
		return 3
	default:
		return 0
	}
}

func Classify(v interface{}) string {
	check := func(x int) bool { return x > 0 && x < 10 }
	switch t := v.(type) {
	case int:
		if check(t) {
			return "small"
		}
	case string:
		return t
	}
	select {
	default:
	}
	return ""
}
