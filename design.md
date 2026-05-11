

# quill-stroke
The stroke algorithm library. It takes real world input points(from pointer or stylus)
as input, and produce a drawable stroke path.

It consists of 2 parts:
- Input Modeler
- stroke Generation

## Input Modeler
Raw Input -> denoising -> Spline generation -> Interpolation

currently only a WILL Input Modeler. It follows Wacoms WILL project's design.
using double expontential algorithm to denoise, and Catmull-Rom spline for the rest.

## Stroke Generation
The basic Idea is represent brush tip with some geometric shape, then the actual
stroke is the set of all points swept by this shape along the path. Normally we would
like to represent this set by its outline, and then tessellate it. For actual realization,
There are generally 2 routes: CPU and GPU
### CPU route
The tip is some polygon, and use union find and polygon merge algorithms to get the outline
polygon, then tessellate it, and pass VAO and VBO to GPU.

### GPU route
Do similar things as the CPU route, but all in GPU, with shader program. the advantage is 
not only speed, but also flexiablity of tip's shape. for example, we may not need tessellation
if it's a round tip, we can just make use of fragment shader to render it.

The disvantage is compatibility with vello.

# quill-canvas
The abstract model of a drawable canvas. Built with bevy's ECS model. Choosing it because it
is easier to integrate with bevy render engine, although haven't fully decided the engine.

We define an abstract model for canvas, it should have all functionalities as a normal canvas
would have, listed below. A renderer would be able to render them by wgpu middleware design
pattern.

The canvas structure:
- Canvas
  - Pages
    - objects(strokes, shape, images, etc)

Pages may or may not be organized linearly. We should allow nested pages(elaborated quick notes),
or pages expand in any directions(a scratch paper). In future, we could also create a 3D view of 
them.

## functionality
- drawing capablity
  - stroke
  - pencil
  - geometric shape
  - eraser
  - image
  - text
- controlling
  - undo/redo
  - selection
  - serialization
  

# quill-app
The frondend of quill, a real notetaking app. For now the general design is slint + bevy, slint
as host, and bevy renders the canvas to a texture.
## Tech Stack
need winit 0.31.x for tablet events to work(https://github.com/rust-windowing/winit/pull/4318).
### wgpu-based GUI framework
- iced?
  I don't like its ui abstractions.
- gpui?
  although it has switched it backend to wgpu, there seems no way to reuse the wgpu device:
  https://github.com/zed-industries/zed/discussions/54236 so impossible to achieve zero copy.
  don't have interest of patching its code either.
- slint?
  The best choice so far, but not favor of a using new markup language. my choice for now.
- masonry/xilem?
  The one I most familiar with. But need to patch its source code to achieve what we want here.
  Should keep up its updates.

### other framework
- flutter
  Is google abandoning it?
- kotlin-based
  good on android and bad for everything else. don't worth the investment.
