# Multi-stage build for Rust NAT-PMP server with multi-arch support
# Versions are centrally managed here for consistency
ARG RUST_VERSION=1.83
ARG ALPINE_VERSION=3.21
FROM --platform=$BUILDPLATFORM rust:${RUST_VERSION}-alpine${ALPINE_VERSION} AS builder

# Build arguments for cross-compilation
ARG TARGETPLATFORM
ARG BUILDPLATFORM
ARG RUST_VERSION
ARG ALPINE_VERSION

# Install build dependencies for static linking
# Pin package versions for reproducible builds
RUN apk add --no-cache \
    musl-dev=1.2.5-r9 \
    libnatpmp-dev=20230423-r0

# Install cross-compilation targets based on target platform
RUN case "$TARGETPLATFORM" in \
    "linux/amd64") \
        rustup target add x86_64-unknown-linux-musl \
        ;; \
    "linux/arm64") \
        rustup target add aarch64-unknown-linux-musl \
        ;; \
    "linux/arm/v7") \
        rustup target add armv7-unknown-linux-musleabihf \
        ;; \
    *) \
        echo "Unsupported platform: $TARGETPLATFORM" && exit 1 \
        ;; \
    esac

# Set target triple based on platform
ENV RUST_TARGET_TRIPLE=""
RUN case "$TARGETPLATFORM" in \
    "linux/amd64") export RUST_TARGET_TRIPLE="x86_64-unknown-linux-musl" ;; \
    "linux/arm64") export RUST_TARGET_TRIPLE="aarch64-unknown-linux-musl" ;; \
    "linux/arm/v7") export RUST_TARGET_TRIPLE="armv7-unknown-linux-musleabihf" ;; \
    esac && echo "RUST_TARGET_TRIPLE=$RUST_TARGET_TRIPLE" >> /etc/environment

# Create app directory
WORKDIR /app

# Copy Cargo files first for better layer caching
COPY Cargo.toml Cargo.lock ./

# Create a dummy main.rs to cache dependencies
RUN mkdir src && echo "fn main() {}" > src/main.rs

# Build dependencies only (this layer gets cached)
RUN . /etc/environment && cargo build --release --target $RUST_TARGET_TRIPLE

# Remove dummy source
RUN rm -rf src

# Copy real source code
COPY src ./src

# Build the actual application  
RUN . /etc/environment && cargo build --release --target $RUST_TARGET_TRIPLE

# Runtime stage - minimal Alpine image
ARG ALPINE_VERSION=3.21
FROM alpine:${ALPINE_VERSION}

# Install only runtime dependencies  
# Pin package versions for reproducible builds
RUN apk add --no-cache \
    libnatpmp=20230423-r0 \
    ca-certificates=20250911-r0

# Create app directory
WORKDIR /app

# Copy the statically linked binary from builder stage (wildcard handles all architectures)
COPY --from=builder /app/target/*/release/natpmp-server .

# Make binary executable
RUN chmod +x natpmp-server

# Expose port
EXPOSE 8080

# Health check
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
    CMD wget --no-verbose --tries=1 --spider http://localhost:8080/health || exit 1

# Run as root to access network capabilities
USER root

# Set default environment variables
ENV NATPMP_BIND_ADDRESS=0.0.0.0
ENV NATPMP_PORT=8080

ENTRYPOINT ["./natpmp-server"]