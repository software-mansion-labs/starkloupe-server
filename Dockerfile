ARG RUST_VERSION=1.80.0

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
  --home "/home" \
  --shell "/sbin/nologin" \
  --uid "10001" \
  appuser

RUN chown -R appuser /home
COPY --from=builder /app/walnut-server/target/release/server /opt/app/server
COPY --from=builder /app/walnut-server/universal-sierra-compiler /opt/app/universal-sierra-compiler
COPY --from=builder /app/walnut-server/binaries /opt/app/binaries
RUN chown -R appuser /opt/app
USER appuser
ENV DATABASE_URL="postgresql://wido:Prankster-Wido@wido-1.cn5qetssppiq.us-east-1.rds.amazonaws.com:5432/walnut"
ENV REDIS_ADDR="redis://widoserver-east-1.h4j9ed.0001.use1.cache.amazonaws.com:6379"
ENV UNIVERSAL_SIERRA_COMPILER="./universal-sierra-compiler"
WORKDIR /opt/app
EXPOSE 3000
ENTRYPOINT ["/opt/app/server"]