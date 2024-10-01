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
cd scripts
./start-db-local.sh
```
Postgres will run on `localhost:1234`.
If you need to change ports, modify `db-docker-compose.yaml` file. If anything changes, assure to update `.env` file with the new values.

2. Build and run the server
```
cp .env.example .env
cargo run --bin server
```
Rust version is specified in `rust-toolchain.toml` file.
Server will listen on `localhost:3000`.
