# GECKO CLI image — used by the `compose` run mode (plan P3). A slim, model-free,
# candle-free build: the ~1.3GB embedding model and TypeDB are NOT baked in (they
# are runtime assets / a separate service), per the packaging invariants.

FROM rust:1-bookworm AS build
WORKDIR /src
COPY . .
# Default features (cyber on, real-embedder OFF): the model is a runtime asset,
# never a build dependency. `js-sandbox` is a wasm-guest crate — exclude it.
RUN cargo build --release --bin gecko --workspace --exclude js-sandbox

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY --from=build /src/target/release/gecko /usr/local/bin/gecko
# Runtime asset cache (models / index / any downloaded TypeDB) lives outside any
# build layer; compose mounts a named volume here.
ENV GECKO_CACHE_DIR=/var/cache/gecko
RUN mkdir -p /var/cache/gecko
ENTRYPOINT ["gecko"]
CMD ["--help"]
