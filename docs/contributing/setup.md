# Development Setup

Set up your environment for Ralph development.

## Prerequisites

### Required

- **Rust 1.75+** — Install via [rustup](https://rustup.rs/)
- **Git** — For version control

### Optional

- **At least one AI CLI** — For integration testing (Claude, Kiro, etc.)
- **tmux** — For TUI testing
- **freeze** — For TUI screenshot capture

## Clone and Build

```bash
# Clone
git clone https://github.com/mikeyobrien/ralph-orchestrator.git
cd ralph-orchestrator

# Build
cargo build

# Build release
cargo build --release
```

## Install Git Hooks

```bash
./scripts/setup-hooks.sh
```

This installs pre-commit hooks that mirror CI Rust checks:

- `./scripts/sync-embedded-files.sh check`
- `cargo fmt --all -- --check`
- `cargo clippy --all-targets --all-features -- -D warnings`
- `./scripts/run-tests.sh`

## Verify Setup

```bash
# Run tests (canonical two-phase nextest gate)
./scripts/run-tests.sh

# Run smoke tests
cargo nextest run -p ralph-core --features recording --test smoke_runner

# Check formatting
cargo fmt --check

# Run clippy
cargo clippy --all-targets --all-features
```

## Project Structure

```
ralph-orchestrator/
├── crates/                    # Cargo workspace crates
│   ├── ralph-proto/           # Protocol types
│   ├── ralph-core/            # Orchestration engine
│   ├── ralph-adapters/        # CLI backends
│   ├── ralph-tui/             # Terminal UI
│   ├── ralph-cli/             # Binary entry point
│   ├── ralph-e2e/             # E2E testing
│   └── ralph-bench/           # Benchmarking
├── presets/                   # Hat collection presets
├── specs/                     # Development specs
├── tasks/                     # Code tasks
├── docs/                      # Documentation
├── scripts/                   # Utility scripts
├── Cargo.toml                 # Workspace config
├── CLAUDE.md                  # AI agent instructions
└── README.md                  # Project overview
```

## Development Workflow

### 1. Create a Branch

```bash
git checkout -b feature/my-feature
```

### 2. Make Changes

Edit code in `crates/`.

### 3. Run Tests

```bash
./scripts/run-tests.sh
```

### 4. Format and Lint

```bash
cargo fmt
cargo clippy --all-targets --all-features
```

### 5. Commit

```bash
git add .
git commit -m "feat: add my feature"
```

### 6. Push and PR

```bash
git push origin feature/my-feature
# Open PR on GitHub
```

## Running Ralph Locally

```bash
# From source
cargo run --bin ralph -- run -p "test prompt"

# With release build
cargo run --release --bin ralph -- run -p "test prompt"

# Direct binary
./target/release/ralph run -p "test prompt"
```

## Testing with Fixtures

Smoke tests use JSONL fixtures:

```bash
# Run smoke tests
cargo nextest run -p ralph-core --features recording --test smoke_runner

# Record a new fixture
cargo run --bin ralph -- run --record-session fixture.jsonl -p "your prompt"
```

## E2E Testing

Requires a live AI backend:

```bash
# Run E2E tests
cargo run -p ralph-e2e -- claude

# Debug mode
cargo run -p ralph-e2e -- claude --keep-workspace --verbose
```

## Debugging

### Enable Diagnostics

```bash
RALPH_DIAGNOSTICS=1 cargo run --bin ralph -- run -p "test"
```

### Debug Logging

```bash
RUST_LOG=debug cargo run --bin ralph -- run -p "test"
```

### GDB/LLDB

```bash
# Build with debug info
cargo build

# Debug
lldb ./target/debug/ralph -- run -p "test"
```

## IDE Setup

### VS Code

Install extensions:

- rust-analyzer
- Even Better TOML
- crates

### IntelliJ IDEA

Install plugins:

- Rust
- TOML

## Common Issues

### Cargo Build Fails

```bash
# Update Rust
rustup update

# Clean and rebuild
cargo clean
cargo build
```

### Tests Fail

```bash
# Run with output (nextest: --no-capture)
cargo nextest run --no-capture

# Run specific test
cargo nextest run -p ralph-core -- test_name
```

### Clippy Errors

```bash
# See all warnings
cargo clippy --all-targets --all-features 2>&1 | less

# Fix automatically
cargo clippy --fix
```

## Next Steps

- Read the [Code Style](style.md) guide
- Learn about [Testing](testing.md)
- See [Submitting PRs](pull-requests.md)
