# Builds orchestrator, api and demo-pipeline; pick one with --target.
FROM docker.io/library/rust:1-bookworm AS build
# The vendored csgoproto build script needs protoc; cargo fetches demoparser via git.
RUN apt-get update && apt-get install -y --no-install-recommends protobuf-compiler libprotobuf-dev \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates crates
COPY vendor vendor
COPY migrations migrations
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked --bins \
    && mkdir -p /out && cp target/release/orchestrator target/release/api target/release/demo-pipeline /out/

FROM docker.io/library/debian:bookworm-slim AS runtime
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* && useradd -u 1000 -m lab
WORKDIR /lab
COPY configs configs
USER lab

FROM runtime AS orchestrator
COPY --from=build /out/orchestrator /usr/local/bin/
ENTRYPOINT ["orchestrator"]
CMD ["run"]

FROM runtime AS api
COPY --from=build /out/api /usr/local/bin/
EXPOSE 8080
ENTRYPOINT ["api"]

FROM runtime AS demo-pipeline
COPY --from=build /out/demo-pipeline /usr/local/bin/
ENTRYPOINT ["demo-pipeline"]
CMD ["run"]
