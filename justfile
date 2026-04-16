# Claude-UI template — dev commands
# Requires: bun, cargo, and the `claude` CLI on PATH.

default:
    @just --list

# Install frontend deps
install:
    bun install

# Frontend-only: Vite dev server on :1420 with /api and /ws proxied to :8080
frontend-dev:
    bun run dev

# Backend-only: Axum server on :8080 (embeds whatever dist/ currently contains)
backend-dev:
    cd backend && cargo run

# Backend release build → backend/target/release/claude-ui-app
backend-release:
    cd backend && cargo build --release

# One-shot production build: frontend bundle + backend release binary
build: install
    bun run build
    cd backend && cargo build --release

# Typecheck frontend + cargo check backend
check:
    bun run check

# All tests (stream decoder, auth, commands)
test:
    cd backend && cargo test

# Format Rust
fmt:
    cd backend && cargo fmt

# Wipe build artifacts
clean:
    rm -rf node_modules dist
    cd backend && cargo clean

# Print local IPs for LAN access reminders
ip:
    @echo "📱 LAN IPs:"
    @ifconfig 2>/dev/null | awk '/inet / && $$2 != "127.0.0.1" {print "  " $$2}' || echo "  (ifconfig unavailable)"
    @echo ""
    @echo "Remember: binding 0.0.0.0 requires APP_AUTH_TOKEN to be set."
