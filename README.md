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