#!/bin/sh
set -eu
[ "$#" -eq 2 ] || { echo "usage: $0 VERSION SHA256SUMS" >&2; exit 2; }
tag=$1 sums=$2 version=${1#v} repo=${PAYDAY_GITHUB_REPOSITORY:-nkrishang/payday}
sum() { value=$(awk -v file="$1" '$2 == file {print $1}' "$sums"); [ -n "$value" ] || { echo "missing checksum: $1" >&2; exit 1; }; printf %s "$value"; }
cat <<EOF
class Payday < Formula
  desc "Payday command-line client"
  homepage "https://github.com/$repo"
  version "$version"
  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/$repo/releases/download/$tag/payday-macos-aarch64.tar.gz"
      sha256 "$(sum payday-macos-aarch64.tar.gz)"
    else
      url "https://github.com/$repo/releases/download/$tag/payday-macos-x86_64.tar.gz"
      sha256 "$(sum payday-macos-x86_64.tar.gz)"
    end
  end
  on_linux do
    on_intel do
      url "https://github.com/$repo/releases/download/$tag/payday-linux-x86_64.tar.gz"
      sha256 "$(sum payday-linux-x86_64.tar.gz)"
    end
  end
  def install
    bin.install "payday"
  end
  test do
    assert_match "payday", shell_output("#{bin}/payday --help")
  end
end
EOF
