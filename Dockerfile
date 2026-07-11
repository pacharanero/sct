# SPDX-FileCopyrightText: 2026 Marcus Baw and Baw Medical Ltd
# SPDX-License-Identifier: AGPL-3.0-or-later

FROM rust:1-bookworm AS builder

WORKDIR /src

# Build from the checked-out repository. The Docker use case is operational
# convenience rather than tiny image optimisation, but the final runtime image
# still contains only the binary and entrypoint. The image is a `sct serve`
# appliance, so it opts out of the default `tui` feature (a container has no
# interactive terminal) and builds just the FHIR server.
COPY . .
RUN cargo build --release --no-default-features --features serve

FROM debian:bookworm-slim AS runtime

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /src/target/release/sct /usr/local/bin/sct
COPY docker/entrypoint.sh /usr/local/bin/sct-docker-entrypoint

RUN chmod +x /usr/local/bin/sct /usr/local/bin/sct-docker-entrypoint \
    && mkdir -p /data /codelists

ENV SCT_DATA_HOME=/data \
    SCT_CODELISTS=/codelists \
    SCT_SERVE_HOST=0.0.0.0 \
    SCT_SERVE_PORT=8080 \
    SCT_FHIR_BASE=/fhir \
    SCT_TRUD_EDITION=uk_monolith \
    SCT_REFSETS=all \
    SCT_LOCALE=en-GB \
    SCT_BOOTSTRAP=true

VOLUME ["/data", "/codelists"]
EXPOSE 8080

ENTRYPOINT ["sct-docker-entrypoint"]
CMD ["serve"]
