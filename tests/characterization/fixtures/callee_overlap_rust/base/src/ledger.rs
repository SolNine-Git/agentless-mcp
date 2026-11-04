use crate::helpers::{led_balance, led_close, led_open, led_post};


pub fn post_ledger(amount: i64) -> i64 {
    let book = led_open(amount);
    let posted = led_post(book);
    let balance = led_balance(posted);
    led_close(balance)
}
