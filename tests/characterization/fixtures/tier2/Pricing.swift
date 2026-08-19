import Foundation

let taxRate = 0.2

func applyTax(amount: Double, rate: Double = taxRate) -> Double {
    // The tax is applied on the whole subtotal.
    return amount * (1 + rate)
}

protocol Priceable {
    func price() -> Double
}

class Invoice: Priceable {
    private let subtotal: Double

    init(subtotal: Double) {
        self.subtotal = subtotal
    }

    func price() -> Double {
        return applyTax(amount: subtotal, rate: taxRate)
    }
}
