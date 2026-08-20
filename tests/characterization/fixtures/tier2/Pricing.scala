import pricing.Money

def applyTax(amount: Double): Double = {
  amount * 1.2
}

class Invoice {
  def price(subtotal: Double): Double = {
    applyTax(subtotal)
  }
}
