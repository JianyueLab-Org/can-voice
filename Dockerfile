# syntax=docker/dockerfile:1

FROM rust:1.80-bookworm AS build
WORKDIR /src

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
       autoconf automake libtool libasound2-dev pkg-config \
    && rm -rf /var/lib/apt/lists/*

COPY Cargo.toml Cargo.lock ./
COPY crates/ ./crates/

RUN cargo build --release -p can-voice-listen

FROM debian:bookworm-slim

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && useradd --system --uid 1000 --create-home --home-dir /home/listen listen \
    && rm -rf /var/lib/apt/lists/*

COPY --from=build /src/target/release/can-voice-listen /usr/local/bin/can-voice-listen

USER 1000:1000
EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/can-voice-listen"]
