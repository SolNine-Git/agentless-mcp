from helpers import led_balance, led_close, led_open, led_post



def post_journal(amount):
    book = led_open(amount)
    if book < 0:
        return 0
    posted = led_post(book * 2)
    balance = led_balance(posted)
    return led_close(balance + 1)
