ARG RUST_VERSION=1.83
ARG ALPINE_VERSION=3.21
FROM rust:${RUST_VERSION}-alpine${ALPINE_VERSION} AS builder

RUN apk add --no-cache \
    musl-dev=1.2.5-r9 \
    libnatpmp-dev=20230423-r0

RUN rustup target add x86_64-unknown-linux-musl
ENV RUST_TARGET_TRIPLE=x86_64-unknown-linux-musl

WORKDIR /app

COPY Cargo.toml Cargo.lock ./
RUN mkdir src && echo "fn main() {}" > src/main.rs
RUN cargo build --release --target $RUST_TARGET_TRIPLE
RUN rm -rf src

COPY src ./src
RUN cargo build --release --target $RUST_TARGET_TRIPLE

ARG ALPINE_VERSION=3.21
FROM alpine:${ALPINE_VERSION}

RUN apk add --no-cache \
    libnatpmp=20230423-r0 \
    ca-certificates=20250911-r0

WORKDIR /app
COPY --from=builder /app/target/*/release/natpmp-server .
RUN chmod +x natpmp-server

EXPOSE 8080
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
    CMD wget --no-verbose --tries=1 --spider http://localhost:8080/health || exit 1

USER root
ENV NATPMP_BIND_ADDRESS=0.0.0.0
ENV NATPMP_PORT=8080

ENTRYPOINT ["./natpmp-server"]