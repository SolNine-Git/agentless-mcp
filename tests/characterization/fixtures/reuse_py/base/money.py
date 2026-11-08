class Price:
    def __init__(self, cents, currency):
        self.cents = cents
        self.currency = currency

    def __eq__(self, other):
        same = isinstance(other, Price) and self.cents == other.cents
        return same and self.currency.upper() == other.currency.upper()
