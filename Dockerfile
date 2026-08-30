# Pin the builder to the same Debian release as the runtime (bookworm). A binary
# built on a newer release can die at exec on the older one the day a dependency
# reaches for a newer glibc/OpenSSL symbol — with no log and, before OPS-04, no
# restart. Keep this in lockstep with the runtime FROM below.
FROM rust:1-bookworm AS builder

# Install build dependencies
RUN apt-get update && \
    apt-get install -y --no-install-recommends \
    pkg-config \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /usr/src/dsec_bot

# Copy manifest files first to leverage Docker layer caching for dependencies
COPY Cargo.toml Cargo.lock ./

# Create a dummy project structure to cache dependency compilation
RUN mkdir src && \
    echo 'fn main() { println!("Dummy main for dependency caching"); }' > src/main.rs

# Build dependencies (this layer will be cached unless Cargo.toml/Cargo.lock changes)
RUN cargo build --locked --release && \
    rm -rf src target/release/deps/dsec_bot*

# Copy the actual source code
COPY src ./src

# Build the actual application
RUN cargo build --locked --release

# Runtime stage - use a minimal image
FROM debian:bookworm-slim

# Create a non-root user for security
RUN groupadd -r dsecbot && useradd --no-log-init -r -g dsecbot dsecbot

# Install runtime dependencies
RUN apt-get update && \
    apt-get install -y --no-install-recommends \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && update-ca-certificates

# Set the working directory
WORKDIR /app

# Copy the application binary from the builder stage
COPY --from=builder /usr/src/dsec_bot/target/release/dsec_bot ./dsec_bot

# Change ownership of the application to the non-root user
RUN chown dsecbot:dsecbot /app/dsec_bot && \
    chmod +x /app/dsec_bot

# Switch to non-root user
USER dsecbot

# Specify the command to run the application
CMD ["./dsec_bot"]