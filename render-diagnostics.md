# Render Diagnostics: Slint, Bevy, WGPU, and Vulkan Validation

Date: 2026-05-16

This document records the current diagnosis of the Vulkan validation errors seen
while embedding the Bevy-backed Quill canvas inside the Slint application. It is
intended as a working diagnostic note, not as a final architecture document.

## 1. Context

Quill currently uses two rendering systems in the same application:

- `quill-app` owns the Slint window and UI.
- `quill-canvas` owns the Bevy render thread and canvas state.
- Bevy renders the canvas into WGPU textures.
- Slint receives those textures as `slint::Image` values and draws them through
  its Skia WGPU renderer.

In debug builds, WGPU/Bevy validation is enabled, and on Vulkan this exposes
layout and synchronization problems around imported textures and swapchain
images.

The important distinction is that there are two related, but separate, image
paths:

- Quill canvas textures imported into Slint as images.
- Slint window swapchain textures used as the final window render target.

Both paths go through Slint's Skia WGPU Vulkan interop code, but they are not the
same texture.

## 2. Current Quill Render Flow

The current high-level flow is:

```text
Slint RenderingSetup
    -> create CanvasRenderer
    -> spawn Bevy render thread
    -> render thread owns Canvas

Slint BeforeRendering
    -> read requested texture size from Slint UI
    -> call CanvasRenderer::render_frame(width, height)
    -> Bevy installs or reuses a render target texture
    -> Bevy render graph runs:
       CanvasPrepareTarget
           -> transition current target to COLOR_TARGET
       CameraDriverLabel
           -> Bevy camera renders canvas into target
       CanvasSwapChain
           -> send finished target to UI thread
    -> Slint Image::try_from(raw WGPU texture clone)
    -> app.set_texture(image)
    -> store previous Slint texture as pending_release

Slint AfterRendering
    -> release pending_release back to CanvasRenderer::free_targets_tx()
    -> Bevy may reuse it in a later frame
```

This is intentionally a delayed-recycle model. The texture that Slint just
displayed is not immediately returned to Bevy in `BeforeRendering`; it is held
until `AfterRendering`, after Slint has drawn the scene for the current frame.

## 3. Implemented Mitigations

Two Quill-side mitigations are already present.

First, old textures are released in Slint `AfterRendering`:

- The previous UI texture is stored in `AppState::pending_release`.
- `pending_release` is sent to `renderer.free_targets_tx()` from
  `RenderingState::AfterRendering`.
- This avoids returning the previous display texture to Bevy before Slint has
  finished drawing the frame that may sample it.

Second, Bevy transitions recycled targets before camera rendering:

- `CanvasPrepareTarget` is inserted before Bevy's camera driver.
- It calls `command_encoder().transition_resources(...)`.
- The target is transitioned to `wgpu::TextureUses::COLOR_TARGET`.
- `CanvasSwapChain` runs after the camera driver and sends the finished target
  back to the UI thread.

These two changes address the expected Quill-side ownership problem: a texture
sampled by Slint should not be immediately reused as Bevy's color attachment,
and a recycled target should be transitioned back before Bevy writes into it.

## 4. Observed Validation Errors

### 4.1 Quill named texture error

The Quill-specific error mentions the named offscreen texture:

```text
VkImage ... [quill-resized-render-target]
expected VK_IMAGE_LAYOUT_COLOR_ATTACHMENT_OPTIMAL
instead current layout is VK_IMAGE_LAYOUT_SHADER_READ_ONLY_OPTIMAL
```

This is important because the image name identifies a Quill offscreen render
target, not merely a Slint window swapchain image.

After the delayed recycle and Bevy transition node were added, this error still
appeared. That means the remaining problem is probably not simply "Bevy forgot
to transition the recycled target before drawing". The Bevy graph now has an
explicit pre-camera transition for that.

### 4.2 Official Slint example errors

The official Slint `slint-host-bevy` example also emits Vulkan validation
errors. Those logs mention unnamed swapchain images, for example:

```text
vkQueuePresentKHR(): PRESENT_AFTER_WRITE hazard detected
vkQueuePresentKHR(): image must be PRESENT_SRC_KHR but is COLOR_ATTACHMENT_OPTIMAL
vkQueueSubmit(): expected COLOR_ATTACHMENT_OPTIMAL but current layout is UNDEFINED
```

These are in the same broad class of Vulkan layout/synchronization problems, but
they are not the exact same texture path as the Quill named texture error:

- Official example: Slint window swapchain/surface image.
- Quill named error: imported WGPU texture displayed by Slint.

This matters diagnostically. If the official example produces swapchain errors,
then some validation noise is already present in upstream Slint/Skia/WGPU Vulkan
interop even without Quill's texture cache.

### 4.3 Unrelated warning

This warning is unrelated to the texture layout issue:

```text
ScreenSpaceAmbientOcclusionPlugin not loaded.
GPU lacks support: Limits::max_storage_textures_per_shader_stage is less than 5.
```

It only means the GPU/backend does not satisfy Bevy's SSAO requirements.

## 5. Source Evidence

The relevant Quill code paths are:

- `quill-app/src/main.rs`
  - `RenderingState::BeforeRendering`
  - `RenderingState::AfterRendering`
  - `AppState::pending_release`
- `quill-canvas/src/render/offscreen.rs`
  - `CanvasPrepareTarget`
  - `CanvasSwapChain`
  - `BackBuffer`

The local Slint source provides the strongest clue.

In `../slint/internal/renderers/skia/wgpu_27_surface/vulkan.rs`,
`make_vulkan_surface(...)` wraps the WGPU swapchain texture for Skia with:

```rust
skia_safe::gpu::vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL
```

The same file's `import_vulkan_texture(...)` also wraps imported WGPU textures
with:

```rust
skia_safe::gpu::vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL
```

In `../slint/internal/renderers/skia/wgpu_27_surface.rs`, after Slint's drawing
callback, imported textures are transitioned with:

```rust
wgpu::TextureUses::RESOURCE
```

On Vulkan, this corresponds to shader-read usage. Therefore the likely sequence
for an imported Quill texture is:

1. Slint imports the WGPU texture into Skia.
2. The Skia backend texture is declared with layout
   `COLOR_ATTACHMENT_OPTIMAL`.
3. Slint later transitions the imported WGPU texture to
   `TextureUses::RESOURCE`.
4. Vulkan sees the image in `SHADER_READ_ONLY_OPTIMAL`.
5. Skia's recorded image information may still expect
   `COLOR_ATTACHMENT_OPTIMAL`.
6. Validation reports a mismatch.

This explains why the named Quill image can still report
`SHADER_READ_ONLY_OPTIMAL` even though Bevy transitions the target back to
`COLOR_TARGET` before its own camera pass.

## 6. Current Hypothesis

The most likely current diagnosis is:

Quill's delayed recycle and Bevy-side transition node are logically necessary,
but they do not fully solve the remaining validation error because Slint's Skia
WGPU 27 Vulkan import path appears to describe sampled imported textures with a
color-attachment layout and then transitions them to resource/shader-read usage.

The official example's swapchain errors suggest this is part of a broader Slint
Skia WGPU Vulkan interop issue, not only a Quill texture-cache bug.

That does not prove Quill is completely innocent, but it changes the next
diagnostic target. The next experiment should focus on the Slint imported
texture path and the Slint swapchain path separately.

## 7. What This Is Not

This is probably not fixed by adding more delay to the Quill texture cache
alone. The delayed-release path already waits until Slint `AfterRendering`.

This is probably not caused by Bevy rendering before the target is transitioned.
The render graph now explicitly transitions the current `BackBuffer` to
`TextureUses::COLOR_TARGET` before the camera driver.

This is not the SSAO warning.

This is not necessarily one single bug. There may be:

- one issue in Slint's swapchain/surface handling;
- another issue in Slint's imported texture handling;
- and Quill-side lifecycle rules that still need to stay correct regardless.

## 8. Next Diagnostics

Run these as separate experiments so the results are easy to interpret.

### 8.1 Compare Quill and official Slint example

Run both with the same backend and validation settings.

Expected interpretation:

- If both show only swapchain-image errors, the base Slint surface path is noisy.
- If Quill additionally shows named `quill-*` texture errors, the imported image
  path needs separate attention.

### 8.2 Patch Slint imported texture layout experimentally

In the local Slint checkout, test whether imported sampled textures should be
declared to Skia with a shader-read-compatible layout instead of
`COLOR_ATTACHMENT_OPTIMAL`.

This should be treated only as a diagnostic patch first. The goal is to answer:

- Does the named `quill-resized-render-target` error disappear?
- Do swapchain errors remain?
- Does rendering still display correctly?

If the named error disappears while swapchain errors remain, then the imported
texture path is confirmed as a separate issue.

### 8.3 Inspect Slint swapchain present path separately

The official example errors involve swapchain images and presentation:

- `PRESENT_AFTER_WRITE`
- present expected `PRESENT_SRC_KHR`
- draw expected `COLOR_ATTACHMENT_OPTIMAL` but saw `UNDEFINED`

These should be diagnosed against Slint's surface path, not Quill's offscreen
texture cache.

### 8.4 Add Quill texture lifecycle logging

Add temporary logs around:

- texture allocation;
- texture reuse from free-target cache;
- Bevy target install;
- Bevy finished-frame send;
- Slint `set_texture`;
- Slint `AfterRendering` release.

Use labels, sizes, and frame counters where available. The purpose is not to fix
the issue directly, but to prove texture ownership order.

### 8.5 Evaluate copy-based display only after import-path diagnosis

A GPU copy path would render into a Bevy-owned color target and copy into a
separate Slint-display texture. That may avoid some layout aliasing by separating
"Bevy writes" from "Slint samples".

However, it adds code and a GPU copy. It is a common technique, but should not be
the first fix until the Slint imported texture layout behavior is understood.

## 9. Acceptance Criteria for Future Fixes

A fix should be evaluated with these outcomes:

- No named `quill-*` Vulkan validation errors during resize, redraw, and idle.
- No panic in Bevy render systems.
- No texture cache reuse before Slint has finished the frame that samples the
  texture.
- If official Slint example still emits swapchain warnings, document those as
  upstream/base Slint behavior and keep them separate from Quill offscreen
  texture correctness.
- Quill should still render correctly on repeated resizes and redraws.

## 10. Current Working Conclusion

The delayed recycle path and Bevy transition node are still the right Quill-side
model. They make the ownership flow sound:

```text
Bevy writes target
    -> Slint samples target
    -> Slint finishes frame
    -> Quill recycles old target
    -> Bevy transitions recycled target before writing again
```

The remaining named validation error points at how Slint imports and transitions
WGPU textures for Skia on Vulkan. The official example confirms that Slint's
base WGPU/Skia/Vulkan path can already trigger related validation errors without
Quill's cache. Further diagnosis should therefore isolate Slint's imported
texture path from its swapchain surface path.
