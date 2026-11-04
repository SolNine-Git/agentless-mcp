use crate::helpers::{led_balance, led_close, led_open, led_post};


pub fn post_journal(amount: i64) -> i64 {
    let book = led_open(amount);
    if book < 0 {
        return 0;
    }
    let posted = led_post(book * 2);
    let balance = led_balance(posted);
    led_close(balance + 1)
}
