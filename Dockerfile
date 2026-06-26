# Stage 1: Build
FROM rust:trixie AS builder
# TODO: add system dependencies if needed
# RUN apt-get update && apt-get install -y --no-install-recommends libfoo-dev && rm -rf /var/lib/apt/lists/*
WORKDIR /build
COPY . .
# BuildKit cache mounts keep the cargo registry and compiled dependencies
# across builds, so only changed crates are recompiled. The caches are not
# part of the image — they persist on the builder between runs.
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/build/target \
    cargo build --release && \
    cp target/release/my-project /usr/local/bin/my-project

# Stage 2: Runtime
FROM debian:trixie-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/*
RUN useradd -m -u 1000 app
COPY --from=builder /usr/local/bin/my-project /usr/local/bin/my-project
USER app
WORKDIR /home/app
EXPOSE 8080
ENTRYPOINT ["my-project"]
