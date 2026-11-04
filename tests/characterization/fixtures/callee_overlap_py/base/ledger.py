from helpers import led_balance, led_close, led_open, led_post



def post_ledger(amount):
    book = led_open(amount)
    posted = led_post(book)
    balance = led_balance(posted)
    return led_close(balance)
