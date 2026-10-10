local function scaled_sum(left, right)
  local total = left + right
  print("sum", total)
  return total * 2
end

local function doubled_total(a, b)
  local sum = a + b
  print("total", sum)
  return sum * 3
end

local function near_miss(left, right)
  local total = left - right
  print("sum", total)
  return total * 2
end
