<?php

namespace App\Billing;

use App\Money\Currency;

const TAX_RATE = 0.2;

function apply_tax(float $amount, float $rate = TAX_RATE): float
{
    // The tax is applied on the whole subtotal.
    return $amount * (1 + $rate);
}

interface Priceable
{
    public function price(): float;
}

class Invoice implements Priceable
{
    private float $subtotal = 0.0;

    public function __construct(float $subtotal)
    {
        $this->subtotal = $subtotal;
    }

    public function price(): float
    {
        return apply_tax($this->subtotal, TAX_RATE);
    }
}
