using Money.Core;

public static decimal ApplyTax(decimal amount) {
    return amount * 1.2m;
}

public class Invoice {
    public decimal Price(decimal subtotal) {
        return ApplyTax(subtotal);
    }
}
