# Running server binary

`cargo run --bin server`

# DB

Read /migrations/README.md


# For Release
If you make changes to the db, run the following to ensure that docker builds succeed

```
cd crates/server
cargo sqlx prepare
```

# Local setup (development)
1. Start Postgres
Assure you have `Docker` installed and running (with `docker compose` support).
```
cd local
./start-db-local.sh
```
(or just `make postgres` from the repo root)
Postgres will run on `localhost:1234`.
If you need to change ports, modify `local/db-docker-compose.yaml` file. If anything changes, assure to update `.env` file with the new values.

2. Install the Universal Sierra Compiler
The server shells out to the `universal-sierra-compiler` binary to compile Sierra classes during simulation and replay, so it must be installed before running the server.
```
make deps
```
This runs `scripts/install-usc.sh`, which downloads the binary into the repo root.
Copy the env file and point the server at the binary through the `UNIVERSAL_SIERRA_COMPILER` variable (matching `local/dev-docker-compose.yaml`):
```
cp .env.example .env
echo "UNIVERSAL_SIERRA_COMPILER=./universal-sierra-compiler" >> .env
```
If the binary is already on your `PATH` under its default name, the env var can be skipped.

Then fill in the four RPC URLs in `.env` (`STARKNET_MAINNET_RPC_URL`,
`STARKNET_SEPOLIA_RPC_URL`, `ETHEREUM_MAINNET_RPC_URL`, `ETHEREUM_SEPOLIA_RPC_URL`)
and set `WALNUT_ADMIN_TOKEN`. The server refuses to start without them. Everything
else in `.env.example` is optional — see the comments there.

3. Build and run the server
```
cargo run --bin server
```
Rust version is specified in `rust-toolchain.toml` file.
Server will listen on `localhost:3000`.
