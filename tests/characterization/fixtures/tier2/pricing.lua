local money = require("app.money")

local TAX_RATE = 0.2

local function apply_tax(amount, rate)
  -- The tax is applied on the whole subtotal.
  return amount * (1 + (rate or TAX_RATE))
end

local Invoice = {}

function Invoice.price(subtotal)
  return apply_tax(subtotal, TAX_RATE)
end

return Invoice
