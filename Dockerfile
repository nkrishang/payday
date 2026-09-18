# syntax=docker/dockerfile:1
FROM rust:1.98.0-bookworm AS builder

WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
RUN cargo build --locked --release -p gum-server -p gum-indexer -p gum-signers \
    && strip target/release/gum-server target/release/gum-indexer target/release/gum-signers

FROM debian:bookworm-slim AS rds-certs
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl \
    && curl --fail --silent --show-error \
      https://truststore.pki.rds.amazonaws.com/global/global-bundle.pem \
      --output /aws-rds-global-bundle.pem \
    && echo "e5bb2084ccf45087bda1c9bffdea0eb15ee67f0b91646106e466714f9de3c7e3  /aws-rds-global-bundle.pem" \
      | sha256sum --check

FROM debian:bookworm-slim AS runtime
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --create-home gateway
COPY --from=rds-certs /aws-rds-global-bundle.pem /usr/local/share/ca-certificates/aws-rds-global-bundle.pem
USER gateway

# gum-server: the public API on 8080, the internal listener (indexer RPC and
# health) on 8081. The same image runs the operator subcommands:
# `gum-server migrate`, `gum-server bus dead`, `gum-server chain resume`.
FROM runtime AS gum-server
COPY --from=builder /app/target/release/gum-server /usr/local/bin/gum-server
EXPOSE 8080 8081
ENTRYPOINT ["gum-server"]

# gum-indexer: read-only chain observer; health on 8080.
FROM runtime AS gum-indexer
COPY --from=builder /app/target/release/gum-indexer /usr/local/bin/gum-indexer
EXPOSE 8080
ENTRYPOINT ["gum-indexer"]

# gum-signers: the only process holding KMS signing permission; health on 8080.
FROM runtime AS gum-signers
COPY --from=builder /app/target/release/gum-signers /usr/local/bin/gum-signers
EXPOSE 8080
ENTRYPOINT ["gum-signers"]
