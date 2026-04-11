# Repository Guidelines

## Project Structure & Module Organization
- Workspace with two crates:
  - Library crate `quill/` (this directory): code in `src/` with modules `modeling.rs`, `noise.rs`, and `types/` (`input_data.rs`, `stroke.rs`). Tests are co-located within each module.
  - App crate `bin/` (`quill-playground`): interactive renderer in `bin/src/main.rs` using Vello + Winit.
- Entrypoints: library API in `src/lib.rs`; app starts in `bin/src/main.rs` with a Winit event loop.

## Build, Test, and Development Commands
- Build all crates: `cargo build --workspace` — compiles library and app.
- Run all tests: `cargo test --workspace` — unit tests only; no GPU required.
- Run playground: `cargo run --bin quill-playground` (or `cd bin && cargo run`).
- Format: `cargo fmt --all` — standard Rust formatting.
- Lint: `cargo clippy --all-targets --all-features -- -D warnings` — keep the tree warning-free.

## Coding Style & Naming Conventions
- Rust 2024 edition; use default `rustfmt` (no overrides).
- Naming: `CamelCase` types/traits; `snake_case` functions, variables, modules.
- Docs: prefer module docs (`//!`); add brief `///` to public items; re-export ergonomic API in `lib.rs`.
- Keep diffs minimal; maintain clear separation between library and app crates.

## Testing Guidelines
- Framework: Rust’s built-in test harness; place unit tests next to modules.
- Scope: fast, deterministic tests for geometry, filters, and modeling; avoid GPU/windowing in tests.
- Run locally: `cargo test --workspace`.

## Commit & Pull Request Guidelines
- Commits: imperative, concise subjects (e.g., "Add stroke smoothing"), with a short body explaining rationale/impact.
- PRs: include summary, motivation, notable changes, and linked issues; add screenshots or a short screen capture if playground behavior changes.
- Pre-merge checklist: `cargo fmt --all`, `cargo clippy ... -D warnings`, `cargo test --workspace`; update docs (including `bin/README.md`) as needed.

## Security & Configuration Tips (Optional)
- Handle GPU/Surface errors (Outdated/Lost) gracefully in app code; reconfigure or skip a frame rather than panic.
- Helpful env vars: `RUST_BACKTRACE=1`, `WINIT_UNIX_BACKEND=x11`, `WGPU_BACKEND=vulkan|gl|metal|dx12`.

