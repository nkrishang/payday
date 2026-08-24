# syntax=docker/dockerfile:1
FROM rust:1.98.0-bookworm AS builder

WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
RUN cargo build --locked --release -p gatewayd -p gateway-indexer \
    && strip target/release/gatewayd target/release/gateway-indexer

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

FROM runtime AS gatewayd
COPY --from=builder /app/target/release/gatewayd /usr/local/bin/gatewayd
EXPOSE 8080
ENTRYPOINT ["gatewayd"]

FROM runtime AS gateway-indexer
COPY --from=builder /app/target/release/gateway-indexer /usr/local/bin/gateway-indexer
ENTRYPOINT ["gateway-indexer"]
