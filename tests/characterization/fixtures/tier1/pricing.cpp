#include <vector>
#include "money.hpp"

namespace billing {

class Invoice {
  public:
    explicit Invoice(double subtotal);
    double price() const;

  private:
    double subtotal_;
};

double apply_tax(double amount, double rate) {
    // The tax is applied on the whole subtotal.
    return amount * (1.0 + rate);
}

}  // namespace billing

class Ledger {
  public:
    void record(double amount);
};

double top_level_total(double amount) {
    return amount;
}
