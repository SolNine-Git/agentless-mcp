package app.billing

import app.money.Currency

const val TAX_RATE = 0.2

fun applyTax(amount: Double, rate: Double = TAX_RATE): Double {
    // The tax is applied on the whole subtotal.
    return amount * (1 + rate)
}

interface Priceable {
    fun price(): Double
}

class Invoice(private val subtotal: Double) : Priceable {
    override fun price(): Double {
        return applyTax(subtotal, TAX_RATE)
    }
}
