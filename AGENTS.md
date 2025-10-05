# Repository Guidelines

## Project Structure & Module Organization
- `src/`: Core runtime modules (`listener`, `connector`, `router`, `stream`, `util`) plus `main.rs` as the Tokio entrypoint.
- `proto/`: gRPC contracts (`rog.proto`) compiled during `build.rs`; regenerate via `cargo build` after editing the proto.
- `demo/`: Ready-to-use TOML configs for local experiments; copy and tweak rather than editing in place.
- `examples/`: Reserved for minimal reproductions or feature showcases—keep self-contained.
- `Cross.toml`, `Dockerfile`, and `.github/workflows/` document release, container, and CI specifics; update in tandem with build changes.

## Build, Test, and Development Commands
- `cargo check`: Fast validation of borrow checker and unresolved items.
- `cargo build --release`: Optimized binary for deployment; emits to `target/release/`.
- `cargo run -- --config <path>`: Execute with a specific config file (alternatively set `ROG_CONFIG`).
- `cargo test`: Run the async integration tests in `src/test.rs`.
- `cargo fmt` / `cargo fmt --check`: Enforce Rustfmt formatting locally and in CI.
- `cargo clippy --all-targets --all-features`: Surface lint suggestions; treat warnings as actionable.

## Coding Style & Naming Conventions
- Rust 2021 edition, four-space indentation, trailing commas, and Rustfmt defaults.
- Modules and files use `snake_case`; types and traits use `PascalCase`; async functions and locals stay `snake_case`.
- Prefer structured errors (`?`, `thiserror`, `Result`) over `unwrap`/`expect` in production paths.
- Keep new modules focused and re-export only what callers need; document public items with `///` summaries.

## Testing Guidelines
- Use `#[tokio::test(flavor = "multi_thread")]` when concurrency is required; default flavor is fine otherwise.
- Structure test helpers under `src/test.rs` or a sibling `tests/` module when scenarios grow.
- Name tests after behavior (`test_socks5_handshake_success`) and assert observable outcomes, not implementation details.
- Run `cargo test` (optionally `-- --nocapture`) before submissions; ensure deterministic ports and clean state.

## Commit & Pull Request Guidelines
- Follow Conventional Commit-style prefixes when practical (`feat:`, `fix:`, `refactor(stream):`), mirroring recent history.
- Keep subject lines imperative and ≤72 characters; expand rationale and risks in the body.
- Reference related issues (`Fixes #123`) and note config/demo changes that affect deployments.
- PRs should list validation steps, include screenshots for UI/observability artifacts, and flag breaking changes or migration instructions.

## Protobuf & Cross-Compilation Notes
- When updating `proto/rog.proto`, rerun `cargo build` to refresh generated code and commit the outputs.
- Cross builds rely on `cross` with protoc bootstrap; ensure new dependencies compile on MUSL targets and update `Cross.toml` pre-build steps as needed.
