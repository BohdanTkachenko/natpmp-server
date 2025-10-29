FROM rust:1.91-alpine3.22 AS builder

RUN apk add --no-cache musl-dev

ENV RUST_TARGET_TRIPLE=x86_64-unknown-linux-musl
RUN rustup target add $RUST_TARGET_TRIPLE

WORKDIR /build

COPY Cargo.toml Cargo.lock src ./
COPY src ./src
RUN cargo build --release --target $RUST_TARGET_TRIPLE && \
    cp target/$RUST_TARGET_TRIPLE/release/natpmp-server natpmp-server && \
    strip natpmp-server && \
    chmod +x natpmp-server 

FROM alpine:3.22

WORKDIR /app
COPY --from=builder /build/natpmp-server .

ENV NATPMP_BIND_ADDRESS=0.0.0.0
ENV NATPMP_PORT=8080
EXPOSE 8080

HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
    CMD wget --no-verbose --tries=1 --spider http://localhost:8080/health || exit 1

ENTRYPOINT ["./natpmp-server"]