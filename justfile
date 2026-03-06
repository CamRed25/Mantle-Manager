# Mantle Manager task runner — install just with: cargo install just

# Default: full pre-merge check
default: check

# Build
build:
    cargo build --workspace

# Clippy
lint:
    cargo clippy --workspace -- -D warnings

# Format check
fmt-check:
    cargo fmt --all -- --check

# Format apply
fmt:
    cargo fmt --all

# Test suite
test:
    cargo test --workspace

# Migration tests (run after data model changes)
test-migrations:
    cargo test -p mantle_core data::migrations

# Check dependency duplicates (run after Cargo.toml changes)
deps:
    cargo tree | grep -E "duplicate" || true

# Full pre-merge protocol (§3.3)
check: build lint fmt-check test

# No-default-features build (must also pass)
check-no-net:
    cargo build --workspace --no-default-features
    cargo test --workspace --no-default-features

# Install the pre-commit hook (idempotent)
install-hooks:
    cp scripts/pre-commit .git/hooks/pre-commit
    chmod +x .git/hooks/pre-commit
    echo "pre-commit hook installed."
