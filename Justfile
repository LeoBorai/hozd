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

# Builds the crate from its packaged tarball, as `cargo publish` verifies it
package:
    cargo package --locked --allow-dirty

# Unit and integration tests
test:
    cargo nextest run --workspace --no-tests=pass

# Creates the next numbered SQL migration in toasty/migrations/ and its entry in
# toasty/history.toml. Applied migrations are append-only: never edit them
db-new-migration name:
    #!/usr/bin/env sh
    set -eu
    last=$(ls toasty/migrations | sed -n 's/^\([0-9]*\)_.*\.sql$/\1/p' | sort -n | tail -1 | sed 's/^0*//')
    id=$(( ${last:-0} + 1 ))
    file=$(printf '%04d_%s.sql' "$id" "{{name}}")
    printf -- '-- Applied migrations are append-only: never edit this file once released.\n-- Separate statements with `toasty::breakpoint` comment lines, as 0001_init.sql does.\n\n' > "toasty/migrations/$file"
    printf '\n[[migrations]]\nid = %d\nname = "%s"\nsnapshot_name = "%04d_snapshot.toml"\n' "$id" "$file" "$id" >> toasty/history.toml
    echo "created toasty/migrations/$file"
