class Router
  def route(kind, flag)
    # A comment line does not count.

    _label = "one
two
three"
    if kind == :a && flag
      1
    elsif kind == :b || flag
      2
    else
      3
    end
  end

  def classify(v)
    result = case v
             when 1, 2 then :low
             when 3 then :mid
             else :high
             end
    label = v > 0 ? "pos" : "neg"
    log(label) unless v.zero?
    result
  rescue ArgumentError
    nil
  end

  def self.pick(value)
    case value
    in [x] if x > 0 then x
    in { name: } unless name.nil? then name
    in _ then nil
    end
  end

  def drain(items)
    items.each { |i| puts i if i.positive? }
    i = 0
    i += 1 while i < 10 and items.any?
    until i.zero?
      i -= 1
    end
    for x in items do puts x end
    Integer(items.first) rescue 0
  end
end
