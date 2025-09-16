# Multi-stage build for optimal size and security
FROM rust:1.89 AS chef
# RUN apk add --no-cache musl-dev
RUN cargo install cargo-chef
WORKDIR /app

FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder
# Copy recipe and build dependencies
COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json

# Copy source code and build application
COPY . .
RUN cargo build --release --bin http

# Final stage - distroless image for maximum security and minimal size
FROM gcr.io/distroless/cc-debian12

# Copy the binary from builder stage
COPY --from=builder /app/target/release/http /usr/local/bin/ical-merger

# Set default environment variables
ENV HOST=0.0.0.0
ENV PORT=3000
ENV HIDE_DETAILS=true

# Expose port
EXPOSE 3000

# Run the application
ENTRYPOINT ["/usr/local/bin/ical-merger"]
