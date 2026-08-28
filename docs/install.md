# Install the Payday CLI

Release archives and the executable are named `payday`. The Cargo package
remains `gateway-cli` for workspace compatibility.

## Linux x86_64 and macOS

This downloads the matching release and verifies it against `SHA256SUMS`:

```sh
curl --fail --proto '=https' --tlsv1.2 https://raw.githubusercontent.com/nkrishang/payday/main/scripts/install.sh | sh
```

For a pinned release and custom destination:

```sh
curl --fail --proto '=https' --tlsv1.2 https://raw.githubusercontent.com/nkrishang/payday/main/scripts/install.sh | PAYDAY_INSTALL_DIR="$HOME/bin" sh -s -- v0.1.0
```

Ensure the destination (default `~/.local/bin`) is on `PATH`. Linux aarch64 is
not currently built. Windows users can download `payday-windows-x86_64.zip` and
verify it with the release's `SHA256SUMS`.

Upgrade an existing Unix installation with `payday upgrade`. It downloads the
release archive and published `SHA256SUMS`, verifies the binary, and replaces
the current executable in place without executing downloaded scripts.

## Cargo

```sh
cargo install --path crates/gateway-cli --locked
```

The package is not claimed to be on crates.io. If published later, its package
name remains `gateway-cli`, while the installed command is `payday`.

## Homebrew

Every release includes a generated, checksum-pinned `payday.rb`. A tap owner
must copy it to `Formula/payday.rb` in their tap; users can then run
`brew install OWNER/TAP/payday`. This repository cannot update an unspecified
external tap without that repository and credentials. The formula only lists
the platforms for which release assets exist.
