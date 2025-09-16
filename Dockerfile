# Multi-stage build for optimal size and security
FROM rust:1.89-alpine AS builder

# Install build dependencies
RUN apk add --no-cache musl-dev pkgconfig openssl-dev

# Set working directory
WORKDIR /app

# Copy dependency files first to leverage Docker layer caching
COPY Cargo.toml Cargo.lock ./

# Create dummy files to build dependencies
RUN mkdir -p src/bin && \
    echo "fn main() {}" > src/bin/http.rs && \
    echo "fn main() {}" > src/bin/cli.rs && \
    echo "pub mod lib { pub mod config; pub mod error; pub mod server; pub mod calendar; pub mod timezone; }" > src/lib.rs && \
    mkdir -p src/lib && \
    echo "use serde::Deserialize; #[derive(Deserialize)] pub struct Config { pub urls: String, pub port: u16, pub host: String, pub hide_details: bool, pub tz_offsets: String, pub future_days_limit: Option<u16> }" > src/lib/config.rs && \
    echo "pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;" > src/lib/error.rs && \
    echo "use crate::lib::config::Config; use crate::lib::error::Result; pub async fn start_server(_config: Config) -> Result<()> { Ok(()) }" > src/lib/server.rs && \
    echo "pub fn merge_calendars() {}" > src/lib/calendar.rs && \
    echo "pub fn parse_timezone() {}" > src/lib/timezone.rs
RUN cargo build --release --bin http
RUN rm -rf src

# Copy source code
COPY src ./src

# Build the actual application
# Touch main.rs to ensure it's rebuilt
RUN touch src/bin/http.rs
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
