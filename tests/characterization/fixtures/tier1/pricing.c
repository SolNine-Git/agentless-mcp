#include <stdio.h>
#include "money.h"

struct Invoice {
    double subtotal;
};

enum Status { DRAFT, SENT };

double apply_tax(double amount, double rate) {
    /* The tax is applied on the whole subtotal. */
    return amount * (1.0 + rate);
}

double *invoice_price(struct Invoice *invoice) {
    return &invoice->subtotal;
}
