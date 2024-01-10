FROM ghcr.io/dattito/rust-alpine-mimalloc:1.75.0 AS chef 
RUN cargo install cargo-chef 
WORKDIR /app

FROM chef as planner
COPY . .
RUN cargo chef prepare  --recipe-path recipe.json

FROM chef as builder
COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json
COPY . .
RUN cargo build --release

FROM scratch AS runtime 
COPY --from=builder /app/target/*-unknown-linux-musl/release/ical-merger /app
EXPOSE 3000
ENTRYPOINT ["/app"]
