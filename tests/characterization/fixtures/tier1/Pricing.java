package app.billing;

import app.money.Currency;

public interface Priceable {
    double price();
}

class Invoice implements Priceable {
    static final double TAX_RATE = 0.2;

    private final double subtotal;

    Invoice(double subtotal) {
        this.subtotal = subtotal;
    }

    public double price() {
        return applyTax(subtotal, TAX_RATE);
    }

    static double applyTax(double amount, double rate) {
        // The tax is applied on the whole subtotal.
        return amount * (1 + rate);
    }
}
