# rbtags

Ruby constant definition index tool / LSP server in Rust.
See [docs/goal.md](docs/goal.md) for motivation.

## Documentation

- [docs/architecture.md](docs/architecture.md) — Module structure, two-phase design, key design decisions
- [docs/ruby-prism-usage.md](docs/ruby-prism-usage.md) — ruby-prism crate API reference
- [docs/lsp-server-usage.md](docs/lsp-server-usage.md) — lsp-server crate usage patterns

## Commands

```sh
cargo test       # Run all tests
cargo clippy     # Lint
cargo run -- dump <path>   # Print definition index for Ruby files
cargo run -- lsp           # Start LSP server (stdio)
```

## Conventions

- Rust edition 2024
- Tests are inline (`#[cfg(test)] mod tests` in each module)
- `resolver.rs` uses ruby-prism's `Visit` trait for full AST traversal
- `indexer.rs` uses manual node matching (only needs structural nodes)

## Development

- Update specs in `docs/specs/` whenever you change the tool behavior
  - Don't mention internal details and focus behaviors that can be observed externally
