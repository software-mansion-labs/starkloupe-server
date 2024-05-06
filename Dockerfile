ARG RUST_VERSION=1.77.1

FROM public.ecr.aws/docker/library/rust:${RUST_VERSION} AS builder
WORKDIR /app/walnut-server
COPY . .
RUN make deps
ENV DATABASE_URL="postgresql://wido:Prankster-Wido@wido-1.cn5qetssppiq.us-east-1.rds.amazonaws.com:5432/walnut"
RUN cargo build --locked --release --bin server

FROM public.ecr.aws/docker/library/rust:${RUST_VERSION} AS final
RUN adduser \
  --disabled-password \
  --gecos "" \
  --home "/nonexistent" \
  --shell "/sbin/nologin" \
  --no-create-home \
  --uid "10001" \
  appuser
COPY --from=builder /app/walnut-server/target/release/server /usr/local/bin
RUN chown appuser /usr/local/bin/server
USER appuser
ENV DATABASE_URL="postgresql://wido:Prankster-Wido@wido-1.cn5qetssppiq.us-east-1.rds.amazonaws.com:5432/walnut"
ENV REDIS_ADDR="redis://widoserver-east-1.h4j9ed.0001.use1.cache.amazonaws.com:6379"
WORKDIR /opt/server
EXPOSE 3000
ENTRYPOINT ["server"]