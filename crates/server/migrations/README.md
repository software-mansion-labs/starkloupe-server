Use `sqlx` command line to create migrations

# Installation

`cargo install sqlx-cli`

# Adding a migration

`sqlx migrate add`

# Check the status of all migrations

`DATABASE_URL=postgres://postgres:postgres@localhost:1234/walnut sqlx migrate info`

# Run all pending migrations

`DATABASE_URL=postgres://postgres:postgres@localhost:1234/walnut sqlx migrate run`
