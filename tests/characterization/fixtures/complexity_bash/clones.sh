#!/usr/bin/env bash

backup() {
  local source="$1"
  cp "$source" "/tmp/backup"
  echo "saved $source"
}

archive() {
  local target="$2"
  cp "$target" "/var/archive"
  echo "stored $target"
}

near_miss() {
  local source="$1"
  cp -p "$source" "/tmp/backup"
  echo "saved $source"
}
