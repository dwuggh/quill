

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

https://arxiv.org/html/2405.00127v2 gives a decent review of current studies towards
stroke generation. However, the main focus is about accurate rendering of beizer path
stroke, while the main focus here is beautiful handwriting strokes. The differences:
1. **different path curves**: stroke paths for vector graphics are predefined bezier curves.
While stroke paths for handwriting must go through given path points, hence we use the
Catmull-Rom spline.
2. **lower requirement for accuracy**: unlike a vector graphic object, we don't need the
rendering to be accurate within 0.25px. we only need the visual effect to be good.
3. **variable width and caps**: we expect the stroke width to vary according to pressure
and angle. With different brush tips, the caps of stroke may change as well. This is the crucial
difference making us cannot just re-use current rendering tools, but have to develop everything
on our own.

Good thing is, the vocabulary remains the same. We can still split the process to path flattening,
stroke expansion, and rendering. We still use offset curves and parallel curves to represent boundaries. 

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

### model of stroke shape
we use offset curve to represent the boundaries of a stroke:
https://www.stat.auckland.ac.nz/~paul/Reports/VWline/offset-xspline/offset-xspline.html

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
### drawing capability
#### stroke
The definition and algorithms are described in previous section. In practice, we can separately
treat fixed width strokes(traditional) and variable width strokes(calligraphical).
##### fixed width strokes
this can be implemented with engine's native method.
note that we also want to customize line shape: solid or dashed
##### variable width strokes
we shall provide customization parameters(from freenote):
- sharpness(of tip)
- pressure sensitivity
#### eraser
erasers are a filled shape that delete things it swept through.
erasers should have 3 modes: pixel-wise, point-wise and element-wise.
we may want erasers only set alpha value of eneties when its moving, and do
post processing after its finished(pointer up).
- pixel-wise
  only delete the "pixel" it cover or sweep by. We also expect it split paths correctly(freenote fails to do that). This is useful with markers, but the hardest one for implementation. 
  TODO idea: we store the eraser path, once its done, compute and split affected entities, and generate a entity-specific eraser area for each entity.
- point-wise
  this is current implementation. it deletes whatever path points it meet. current implementation
  is only this style.
- element-wise
  this is easy. delete the object when the eraser intersects with it.
- geometric shape
    shapes are predefined path with fixed-width strokes.
- marker
    markers are fixed-width strokes, but need to be careful with its alpha and color mixing
- TODO pencil
  we may want the blast style in WILL.
- ruler
  rulers are an object to restrict drawing, i.e. restrict path points.
- image
- text
### controlling
#### undo/redo
We need to design the commands.
TODO can be use some mature crates here?
#### selection
the chanllenge is how to "semantically" select contents.
#### section marker and hyperlink
We can set properties to entities. for example, a section system, or just a tag like org-mode.
We also want to provide references.

#### serialization
TODO this is least priority. ignore it for now.
  
### page system
quill-canvas should not make assumptions to page structure.
TODO do we need a page system here in the first place?
pros: 
- leveled entity structure
- performant hit test
cons:
- more complex logic for drawing, selecting and moving
- performance optimization may not worth it

I believe the hit test is fast enough, so no need to restrict entities to pages.


# quill-app
The frondend of quill, a real notetaking app. For now the general design is slint + bevy, slint
as host, and bevy renders the canvas to a texture.
## Tech Stack
need winit 0.31.x for tablet events to work(https://github.com/rust-windowing/winit/pull/4318).
slint has a ongoing pr using that.
### wgpu-based GUI framework
- slint?
  The best choice so far, but not favor of a using new markup language. my choice for now.
- iced?
  I don't like its ui abstractions, so no.
- gpui?
  although it has switched it backend to wgpu, there seems no way to reuse the wgpu device:
  https://github.com/zed-industries/zed/discussions/54236 so impossible to achieve zero copy.
  don't have interest of patching its code either.
- masonry/xilem?
  The one I most familiar with. But need to patch its source code to achieve what we want here.
  Should keep up its updates.

### other framework
- flutter
  Is google abandoning it?
- kotlin-based
  good on android and bad for everything else. don't worth the investment.

