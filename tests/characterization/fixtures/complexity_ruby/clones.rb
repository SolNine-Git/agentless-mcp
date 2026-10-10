def scaled_sum(left, right)
  total = left + right
  puts "sum #{total}"
  total * 2
end

def doubled_total(a, b)
  sum = a + b
  puts "total #{sum}"
  sum * 3
end

def near_miss(left, right)
  total = left - right
  puts "sum #{total}"
  total * 2
end
