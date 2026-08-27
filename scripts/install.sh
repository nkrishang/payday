#!/bin/sh
set -eu
repo=${PAYDAY_GITHUB_REPOSITORY:-nkrishang/payday}
install_dir=${PAYDAY_INSTALL_DIR:-"$HOME/.local/bin"}
version=${1:-${PAYDAY_VERSION:-latest}}
case $(uname -s) in Linux) os=linux;; Darwin) os=macos;; *) echo "payday: unsupported OS" >&2; exit 1;; esac
case $(uname -m) in x86_64|amd64) arch=x86_64;; arm64|aarch64) arch=aarch64;; *) echo "payday: unsupported architecture" >&2; exit 1;; esac
[ "$os-$arch" != linux-aarch64 ] || { echo "payday: Linux aarch64 releases are not available" >&2; exit 1; }
command -v curl >/dev/null || { echo "payday: curl is required" >&2; exit 1; }
if command -v sha256sum >/dev/null; then
  check_checksum() { sha256sum -c; }
elif command -v shasum >/dev/null; then
  check_checksum() { shasum -a 256 -c; }
else
  echo "payday: SHA-256 tool required" >&2; exit 1
fi
tmp=$(mktemp -d 2>/dev/null || mktemp -d -t payday)
trap 'rm -rf "$tmp"; [ -z "${staged:-}" ] || rm -f "$staged"' EXIT HUP INT TERM
asset="payday-$os-$arch.tar.gz"
if [ "$version" = latest ]; then base="https://github.com/$repo/releases/latest/download"; else case $version in v*) ;; *) version=v$version;; esac; base="https://github.com/$repo/releases/download/$version"; fi
curl --fail --location --proto '=https' --tlsv1.2 --retry 3 -o "$tmp/$asset" "$base/$asset"
curl --fail --location --proto '=https' --tlsv1.2 --retry 3 -o "$tmp/SHA256SUMS" "$base/SHA256SUMS"
expected=$(grep "  $asset\$" "$tmp/SHA256SUMS" || true)
[ -n "$expected" ] || { echo "payday: checksum missing for $asset" >&2; exit 1; }
(cd "$tmp" && printf '%s\n' "$expected" | check_checksum)
tar -xzf "$tmp/$asset" -C "$tmp"; [ -f "$tmp/payday" ] || { echo "payday: invalid archive" >&2; exit 1; }
mkdir -p "$install_dir"
staged="$install_dir/.payday.new.$$"
install -m 0755 "$tmp/payday" "$staged"
mv -f "$staged" "$install_dir/payday"
echo "payday installed to $install_dir/payday"
