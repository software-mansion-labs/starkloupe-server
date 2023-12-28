ARG RUST_VERSION=1.74.1

FROM rust:${RUST_VERSION}-slim-bookworm AS builder
WORKDIR /app
COPY . .
ENV DATABASE_URL="postgresql://wido:Prankster-Wido@wido-1.cn5qetssppiq.us-east-1.rds.amazonaws.com:5432/walnut"
RUN apt-get update && apt-get install -y libssl-dev
RUN cargo build --locked --release --bin server

FROM debian:bookworm-slim AS final
RUN adduser \
  --disabled-password \
  --gecos "" \
  --home "/nonexistent" \
  --shell "/sbin/nologin" \
  --no-create-home \
  --uid "10001" \
  appuser
RUN apt-get update && apt install -y openssl
RUN apt-get install -y --no-install-recommends ca-certificates
RUN update-ca-certificates
COPY --from=builder /app/target/release/server /usr/local/bin
RUN chown appuser /usr/local/bin/server
USER appuser
ENV DATABASE_URL="postgresql://wido:Prankster-Wido@wido-1.cn5qetssppiq.us-east-1.rds.amazonaws.com:5432/walnut"
ENV REDIS_ADDR="redis://widoserver-east-1.h4j9ed.0001.use1.cache.amazonaws.com:6379"
WORKDIR /opt/server
EXPOSE 3000
ENTRYPOINT ["server"]
