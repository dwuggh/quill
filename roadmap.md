read design.md first.

# checklist
To AI: for easy and tasks, you can do it directly, then check it; for midium tasks, some design maybe needed; for hard tasks, write your plan to this file first, after I walkthrough it, you can start implement according to the plan.
## easy
- [x] remove custom Vec2 in quill-stroke, use plain x and y, or a slice type. remove its builders, only construct with specific construction
- [x] move all render-related stuff to the QuillRenderPlugin
- [x] use peniko's color type instead of a [f32; 4].
- [x] remove _pad in RoundTip, and other config types. the gpu alignment should be done separately.
- [x] enable logs in quill-app

## midium
- [ ] make point-wise eraser split the path when pointer up, and set alphas when pointer move.
- [ ] display a eraser mesh according to its shape. it should be a white filled shape width borders, optionally shadowed.
- [ ] consider which systems can be executed in parallel.
- [ ] a better source code file structure for quill-canvas

## hard
- [x] unify exposed API in quill-canvas, so that we don't need to use bevy in quill-app, but only a wgpu middleware style rendering with methods controlling the canvas.
- [ ] add fixed width style strokes. we may want to use a separate BrushTip instead of changing pressure_mode, because it is a whole different pipeline. this would be the foundation of shape.
- [ ] undo-redo system. Use the simple command record and replay style.
- [ ] add pages. they are special images that always placed on bottom.


ActiveEntity
mode.apply()