#
# Base container (with sccache and cargo-chef)
#
# - https://github.com/mozilla/sccache
# - https://github.com/LukeMathWalker/cargo-chef
#
# Based on https://depot.dev/blog/rust-dockerfile-best-practices
#
FROM rust:1.87.0 AS base

ARG FEATURES
ARG RELEASE=true

RUN cargo install sccache --version ^0.9
RUN cargo install cargo-chef --version ^0.1

RUN apt-get update \
    && apt-get install -y clang libclang-dev

ENV CARGO_HOME=/usr/local/cargo
ENV RUSTC_WRAPPER=sccache
ENV SCCACHE_DIR=/sccache

#
# Planner container (running "cargo chef prepare")
#
FROM base AS planner
WORKDIR /app

COPY . .

RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=$SCCACHE_DIR,sharing=locked \
    cargo chef prepare --recipe-path recipe.json

#
# Builder container (running "cargo chef cook" and "cargo build --release")
#
FROM base AS builder
WORKDIR /app
# Default binary filename
ARG ROLLUP_BOOST_BIN="rollup-boost"
COPY --from=planner /app/recipe.json recipe.json

RUN --mount=type=cache,target=$SCCACHE_DIR,sharing=locked \
    PROFILE_FLAG=$([ "$RELEASE" = "true" ] && echo "--release" || echo "") && \
    cargo chef cook $PROFILE_FLAG --recipe-path recipe.json

COPY . .

RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=$SCCACHE_DIR,sharing=locked \
    PROFILE_FLAG=$([ "$RELEASE" = "true" ] && echo "--release" || echo "") && \
    TARGET_DIR=$([ "$RELEASE" = "true" ] && echo "release" || echo "debug") && \
    cargo build $PROFILE_FLAG --features="$FEATURES" --package=${ROLLUP_BOOST_BIN}; \
    cp target/$TARGET_DIR/${ROLLUP_BOOST_BIN} /tmp/final_binary

#
# Runtime container
#
FROM gcr.io/distroless/cc-debian12
WORKDIR /app

ARG ROLLUP_BOOST_BIN="rollup-boost"
COPY --from=builder /tmp/final_binary /usr/local/bin/rollup-boost

ENTRYPOINT ["/usr/local/bin/rollup-boost"]
