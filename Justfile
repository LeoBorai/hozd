default:
    @echo "No default task specified."
    just --list

# Runs the Development Kit Container
dkc:
    docker pull ghcr.io/leoborai/dkc:latest
    docker run -it --rm \
        -v $(pwd):/app \
        -w /app \
        ghcr.io/leoborai/dkc:latest

fmt:
    cargo clippy --fix --workspace --allow-dirty --allow-staged
    cargo fmt

# Dependency check, as CI runs it: advisories, licenses, bans, sources; warnings are errors (needs cargo-deny)
deny:
    cargo deny check -D warnings

# Updates Cargo.lock to the latest compatible versions, then checks them (needs cargo-deny)
deps-update:
    cargo update
    cargo deny check -D warnings

# Runs hozd
run *args:
    cargo run -- {{args}}

# Unit and integration tests
test:
    cargo nextest run --workspace --no-tests=pass

# Creates a new SQL migration in migrations/ (needs sqlx-cli). Applied migrations are append-only: never edit them
db-new-migration name:
    sqlx migrate add --sequential {{name}}

# Regenerates the committed .sqlx/ offline query metadata (needs sqlx-cli). Run after changing queries or migrations
db-prepare:
    rm -f target/sqlx-prepare.sqlite3
    DATABASE_URL=sqlite://target/sqlx-prepare.sqlite3 sqlx database create
    DATABASE_URL=sqlite://target/sqlx-prepare.sqlite3 sqlx migrate run
    DATABASE_URL=sqlite://target/sqlx-prepare.sqlite3 cargo sqlx prepare -- --all-targets
