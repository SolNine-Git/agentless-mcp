local M = {}

local function clamp(v, lo, hi)
  -- A comment line does not count.

  local label = [[one
two
three]]
  if v < lo then
    return lo
  elseif v > hi then
    return hi
  end
  return v
end

function M.pick(xs, flag)
  local best = flag and xs[1] or nil
  for i = 1, #xs do
    while xs[i] == nil do
      i = i + 1
    end
  end
  for _, v in ipairs(xs) do
    repeat
      v = v - 1
    until v <= 0 or flag
  end
  return best
end

M.handler = function(ev)
  return ev and ev.ok
end

local a, b = function() return 1 end, 2

return M
