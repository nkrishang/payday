#!/bin/sh
set -eu
tmp=$(mktemp -d 2>/dev/null || mktemp -d -t payday-test); trap 'rm -rf "$tmp"' EXIT HUP INT TERM
for asset in payday-macos-aarch64.tar.gz payday-macos-x86_64.tar.gz payday-linux-x86_64.tar.gz; do printf '%064d  %s\n' 0 "$asset" >> "$tmp/SHA256SUMS"; done
scripts/generate-homebrew-formula.sh v1.2.3 "$tmp/SHA256SUMS" > "$tmp/payday.rb"
grep -q 'version "1.2.3"' "$tmp/payday.rb"
grep -q 'payday-linux-x86_64.tar.gz' "$tmp/payday.rb"
cargo metadata --no-deps --format-version 1 | grep -q '"name":"payday"'
