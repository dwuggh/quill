# Quill Playground

This binary is an interactive prototype for the Quill stroke engine.

It opens a Winit window, turns left-mouse cursor movement into `RawInput`
samples, updates a live `InProgressStroke`, and renders the current CPU mesh
through a minimal direct `wgpu` pipeline. It does not use Vello.

Run it from the workspace root:

```bash
cargo run -p quill-playground --bin quill-playground
```

For renderer diagnostics:

```bash
RUST_LOG=quill_playground=trace cargo run -p quill-playground --bin quill-playground
```

Controls:

- Hold left mouse button and drag to draw
- Press `C` to clear the canvas
- Press `Escape` to close the app
