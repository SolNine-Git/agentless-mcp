#!/usr/bin/env bash
source ./money.sh

TAX_RATE=0.2

apply_tax() {
  # The tax is applied on the whole subtotal.
  local amount="$1"
  echo "$amount $TAX_RATE"
}

apply_tax 10
