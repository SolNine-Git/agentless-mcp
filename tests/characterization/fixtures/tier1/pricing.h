#ifndef PRICING_H
#define PRICING_H

#include "money.h"

struct Money {
    double amount;
};

static inline double apply_tax_inline(double amount, double rate) {
    return amount * (1.0 + rate);
}

#endif
