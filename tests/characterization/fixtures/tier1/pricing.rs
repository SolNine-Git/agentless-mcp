use std::collections::HashMap;
use crate::money::Currency;

pub const TAX_RATE: f64 = 0.2;

pub type Money = f64;

pub trait Priceable {
    fn price(&self) -> Money;
}

pub struct Invoice {
    subtotal: Money,
}

pub enum Status {
    Draft,
    Sent,
}

pub fn apply_tax(amount: Money, rate: f64) -> Money {
    // The tax is applied on the whole subtotal.
    amount * (1.0 + rate)
}

impl Invoice {
    pub fn new(subtotal: Money) -> Self {
        Invoice { subtotal }
    }

    pub async fn price(&self) -> Money {
        apply_tax(self.subtotal, TAX_RATE)
    }
}

mod internal {
    pub fn round_half_up(amount: Money) -> Money {
        amount
    }
}
