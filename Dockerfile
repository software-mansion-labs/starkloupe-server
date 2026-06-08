# Is using glibc 2.40 (check with `ldd --version`)
FROM ubuntu:25.04 AS builder

# Install required dependencies
RUN apt-get update && apt-get install -y \
  curl \
  build-essential \
  libssl-dev \
  pkg-config

# Install Rust via rustup
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | bash -s -- -y
ENV PATH="/root/.cargo/bin:${PATH}"

WORKDIR /app/walnut-server
COPY . .

RUN make deps
ENV SQLX_OFFLINE=true
RUN rustup install 1.91.0 && rustup default 1.91.0
RUN cargo build --locked --release --bin server

FROM ubuntu:25.04 AS final

RUN apt-get update && apt-get install -y --no-install-recommends \
  openssl \
  curl \
  build-essential \
  ca-certificates \
  git \
  bash && \
  apt-get clean && rm -rf /var/lib/apt/lists/*

# Install Rust via rustup
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | bash -s -- -y
ENV PATH="/root/.cargo/bin:${PATH}"
RUN rustup install 1.91.0 && rustup default 1.91.0

WORKDIR /opt/app

# Copy the built artifacts from the builder stage
COPY --from=builder /app/walnut-server/target/release/server /opt/app/server
COPY --from=builder /app/walnut-server/universal-sierra-compiler /opt/app/universal-sierra-compiler

RUN mkdir -p /opt/app/binaries

EXPOSE 3000

RUN chmod +x /opt/app/server

# Set the entrypoint to the compiled Rust binary
ENTRYPOINT ["/opt/app/server"]
