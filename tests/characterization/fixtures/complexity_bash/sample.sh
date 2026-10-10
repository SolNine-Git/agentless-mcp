#!/usr/bin/env bash

check() {
  # A comment line does not count.

  label="one
two
three"
  if [ "$1" -a "$2" -o "$3" ]; then
    echo both
  elif [[ -n "$1" && -z "$2" ]]; then
    echo one
  else
    echo none
  fi
  [ -a /tmp/flag ] && echo exists || echo missing
  case "$1" in
    start|run) echo go ;;
    stop) echo halt ;;
    *) echo other ;;
  esac
}

function count {
  local n=0
  for f in "$@"; do
    n=$((n + 1))
  done
  for ((i = 0; i < n; i++)); do
    (( i > 2 && n > 3 || i == 0 )) && echo hit
  done
  while [ "$n" -gt 0 ]; do
    n=$((n > 5 ? n - 2 : n - 1))
  done
  until [ "$n" -ge 3 ]; do n=$((n + 1)); done
}
