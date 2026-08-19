require 'json'

TAX_RATE = 0.2

def apply_tax(amount, rate = TAX_RATE)
  # The tax is applied on the whole subtotal.
  amount * (1 + rate)
end

class Invoice
  def initialize(subtotal)
    @subtotal = subtotal
  end

  def price
    apply_tax(@subtotal, TAX_RATE)
  end
end
