FROM lukemathwalker/cargo-chef:latest-rust-1 AS chef
WORKDIR /app

FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder 
COPY --from=planner /app/recipe.json recipe.json
# Build dependencies - this is the caching Docker layer!
RUN cargo chef cook --release --recipe-path recipe.json
# Build application
COPY . .
RUN cargo build --release --bin server

# We do not need the Rust toolchain to run the binary!
FROM debian:bookworm-slim AS runtime
WORKDIR /app
RUN apt-get update && apt install -y openssl
COPY --from=builder /app/target/release/server /usr/local/bin

# TODO: Change password and remove from here
ENV DATABASE_URL="postgresql://wido:Prankster-Wido@wido-1.cn5qetssppiq.us-east-1.rds.amazonaws.com:5432/walnut"

EXPOSE 3000
ENTRYPOINT ["/usr/local/bin/server"]