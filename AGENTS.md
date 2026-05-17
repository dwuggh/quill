# Repository Guidelines

## Project Structure & Module Organization

This is a Rust workspace with three crates:

- `quill-stroke/`: low-level stroke data, input modeling, brush types, and shader literals. WGSL files live in `quill-stroke/src/shader/` and should be loaded with `include_str!`.
- `quill-canvas/`: canvas scene state, input routing, eraser/stroke systems, Bevy render integration, and the `CanvasRenderer` host boundary.
- `quill-app/`: Slint + Bevy application. GUI files live in `quill-app/ui/`; avoid `slint!` macros for app UI.
- `design.md`: The design document.

Tests are colocated with the modules they cover. Keep design notes in separate docs, but do not rewrite design documents as part of routine code changes unless explicitly requested.

## Build, Test, and Development Commands

- `cargo check --workspace`: fast compile check for all crates.
- `cargo build --workspace`: build the complete workspace.
- `cargo test --workspace`: run all unit and doc tests.
- `cargo run -p quill-app --bin quill`: run the Slint-hosted app.
- `cargo fmt --all`: format all Rust code.
- `cargo clippy --all-targets --all-features -- -D warnings`: lint strictly when preparing a PR.

## Coding Style & Naming Conventions

Use Rust 2024 and standard `rustfmt`. Use `CamelCase` for types and traits, `snake_case` for functions, modules, variables, and fields. Prefer methods and associated functions when behavior belongs to a type; avoid loose helper functions for type-owned behavior. Keep shader code in `.wgsl` files, not large inline strings. Keep diffs scoped and avoid unrelated refactors.

Add proper comments for public methods. Also comment important structs, important fields, important private functions, and non-obvious steps inside functions. Comments should explain purpose, ownership, invariants, or pipeline flow; skip comments that merely repeat the code.

## Testing Guidelines

Use Rust’s built-in test harness. Add focused tests beside the changed module, especially for input modeling, stroke geometry, eraser splitting, and render data preparation. Avoid GPU/window tests in normal unit tests. Test names should describe behavior, for example `cached_inputs_remain_bounded_during_long_stroke`.

## Commit & Pull Request Guidelines

The existing history is terse; prefer clearer imperative commits such as `Add eraser stroke splitting`. PRs should include a short summary, motivation, notable behavior changes, and linked issues when relevant. Include screenshots or short captures for UI, stroke rendering, or input behavior changes. Before review, run `cargo fmt --all`, `cargo check --workspace`, and `cargo test --workspace`.

## Security & Configuration Tips

Handle GPU surface and texture failures without panicking where practical. Useful diagnostics include `RUST_BACKTRACE=1`, `WINIT_UNIX_BACKEND=x11`, and `WGPU_BACKEND=vulkan|gl|metal|dx12`.
