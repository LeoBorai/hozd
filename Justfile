default:
    @echo "No default task specified."
    just --list

# Dependency check, as CI runs it: warnings are errors (needs cargo-deny)
deny:
    cargo deny check -D warnings

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

# Runs the SwiftUI app (macOS with the Xcode command line tools)
dev-macos:
    cargo build -p sealpdf-ffi
    swift run --package-path src/macos -Xlinker -L{{justfile_directory()}}/target/debug

# Release build of the macOS app: the Rust library, then the Swift executable linked to it
build-macos:
    cargo build --release -p sealpdf-ffi
    swift build -c release --package-path src/macos -Xlinker -L{{justfile_directory()}}/target/release

# Bundle the app into a standalone target/macos/SealPDF.app (macOS only, see src/macos/Scripts/bundle.sh)
bundle-macos: build-macos
    src/macos/Scripts/bundle.sh

# Unit and integration tests
test:
    cargo nextest run --workspace --no-tests=pass
