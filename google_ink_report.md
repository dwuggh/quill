# Technical Report on `google/ink`: Structure, Abstract Model, Pipeline, and Algorithms

## 1. Overview

`google/ink` is a **freehand stroke generation library**. Its purpose is to transform a stream of raw pen/touch input into a **smoothed, brush-shaped, mesh-based vector stroke** that can be incrementally updated during live drawing.

At a high level, the repository is organized around the dependency chain

\[
\texttt{types,color,geometry} \rightarrow \texttt{brush} \rightarrow \texttt{strokes} \rightarrow \texttt{rendering/storage}.
\]

The computational core is concentrated in the following modules:

- `StrokeInputModeler`
- `BrushTipModeler`
- `BrushTipShape`
- `BrushTipExtruder`
- `StrokeShapeBuilder`
- `InProgressStroke`

These define the true algorithmic pipeline:

\[
\boxed{
\text{raw inputs}
\;\to\;
\text{modeled inputs}
\;\to\;
\text{behavior modifiers}
\;\to\;
\text{tip states}
\;\to\;
\text{tip shapes}
\;\to\;
\text{triangle mesh}
}
\]

The library is **incremental** throughout:
- raw input can arrive in batches,
- predicted inputs can be replaced,
- unstable geometry can be rolled back,
- stable geometry can be retained.

That incremental structure is the central design idea of the repository.

---

## 2. Abstract Mathematical Model

## 2.1 Raw inputs

A raw input sample may be modeled as

\[
r_i = \bigl(p_i,\ t_i,\ \pi_i,\ \theta_i,\ \phi_i,\ u_i,\ \mathrm{tool}_i \bigr),
\]

where:

- \(p_i \in \mathbb{R}^2\): pointer position,
- \(t_i \in \mathbb{R}_{\ge 0}\): elapsed time since the stroke started,
- \(\pi_i\): pressure,
- \(\theta_i\): tilt,
- \(\phi_i\): orientation,
- \(u_i\): stroke-unit-to-physical-unit scale (if known),
- \(\mathrm{tool}_i\): input device type (stylus, touch, mouse, etc.).

The implementation handles both **real** inputs and **predicted** inputs.

---

## 2.2 Modeled stroke inputs

The input modeler converts the raw sequence into a modeled sequence

\[
m_k = \bigl(x_k,\ \tau_k,\ \pi_k,\ \theta_k,\ \phi_k,\ d_k,\ v_k,\ a_k \bigr),
\]

where:

- \(x_k \in \mathbb{R}^2\): modeled position,
- \(\tau_k\): modeled timestamp,
- \(d_k\): traveled distance along the modeled path,
- \(v_k \in \mathbb{R}^2\): modeled velocity,
- \(a_k \in \mathbb{R}^2\): modeled acceleration.

Each modeled input is classified as either:

- **stable**: guaranteed not to change in future updates,
- **unstable**: may be recomputed when more input arrives.

This distinction is the key to live stroke prediction.

---

## 2.3 Brush behavior program

The brush is not just a static geometric template. It also contains a **behavior graph** that maps modeled input features into brush-tip modifications.

Abstractly, let

\[
z_k = \mathcal{B}(m_k, \text{stroke state}, \text{brush parameters})
\]

denote the vector of brush modifiers produced from the modeled input \(m_k\).

The implementation stores this graph as a **flattened post-order traversal**, which behaves operationally like a stack machine over nullable finite floating-point values.

---

## 2.4 Brush tip state

For each modeled input, the brush logic produces a visible tip state

\[
s_k = \bigl(c_k,\ w_k,\ h_k,\ \rho_k,\ \sigma_k,\ \alpha_k,\ \beta_k,\ \chi_k \bigr),
\]

where:

- \(c_k \in \mathbb{R}^2\): tip center,
- \(w_k, h_k\): width and height,
- \(\rho_k\): corner rounding,
- \(\sigma_k\): slant,
- \(\alpha_k\): rotation,
- \(\beta_k\): pinch,
- \(\chi_k\): color/opacity/texture parameters.

Each tip state is again classified as either:

- **fixed**: safe to permanently commit into the mesh,
- **volatile**: must remain undoable because it may change.

---

## 2.5 Tip shapes and mesh

Each tip state becomes an analytic geometric shape

\[
Q_k = \mathcal{S}(s_k),
\]

and the final stroke mesh is produced by an extrusion operator

\[
M = \mathcal{E}(Q_1,Q_2,\dots,Q_n).
\]

The specific shape representation used by the repository is important:

\[
Q_k = \operatorname{conv}\!\left(\bigcup_{j=1}^{m_k} C_{k,j}\right),
\qquad 1 \le m_k \le 4,
\]

that is, the tip shape is represented as the **convex hull of 1 to 4 perimeter circles**.

This gives a representation rich enough to support slanted, pinched, rounded, rotated brush tips while remaining structured enough for robust tangent construction.

---

## 3. End-to-End Pipeline

The actual stroke generation pipeline can be described as follows.

### Step 1: Receive raw and predicted inputs

The public `InProgressStroke` object accepts:

- new real inputs,
- a current predicted input tail.

Prediction is replaceable, so downstream geometry must support rollback.

### Step 2: Model the raw sequence

The `StrokeInputModeler` performs:

- smoothing,
- denoising,
- upsampling,
- derivative estimation,
- stable/unstable segmentation.

### Step 3: Evaluate the brush behavior graph

For each modeled input, the brush behavior engine computes modifiers based on features such as:

- pressure,
- tilt,
- orientation,
- speed,
- direction,
- traveled distance,
- elapsed time,
- time since stroke end,
- acceleration.

### Step 4: Generate brush tip states

The `BrushTipModeler` uses the base brush-tip parameters plus the modifiers to produce tip states.

These are classified as fixed or volatile.

### Step 5: Convert tip states to analytic tip shapes

Each tip state is mapped into a shape represented by perimeter circles and their convex hull.

### Step 6: Extrude the shapes into mesh geometry

The `BrushTipExtruder` connects consecutive tip shapes into a mesh, inserting caps, turn geometry, and breaks where needed.

### Step 7: Package for rendering and querying

The mutable mesh can later be frozen as a `PartitionedMesh`, which supports fast geometric queries using a static R-tree.

---

## 4. Algorithm 1: Sliding-Window Input Modeling

This is the main signal-processing stage in the repository.

## 4.1 Modeled timestamps

Suppose the raw input times are

\[
t_0 < t_1 < \cdots < t_n.
\]

The modeler always creates a modeled input at each raw timestamp. It may also insert additional modeled timestamps if the raw interval is too large.

Let \(\Delta_u\) be the desired upsampling period. Then for the interval \([t_{i-1}, t_i]\), the number of divisions is

\[
N_i = \min\!\left(
\left\lceil \frac{t_i - t_{i-1}}{\Delta_u} \right\rceil,
100
\right).
\]

Then the inserted modeled times are

\[
\tau_{i,j} = t_{i-1} + \frac{j}{N_i}(t_i-t_{i-1}),
\qquad j=1,\dots,N_i-1.
\]

The hard cap of \(100\) exists to prevent pathological memory growth in abnormal timing conditions.

---

## 4.2 Symmetric sliding-window averaging

Let the configured smoothing window size be \(H\), and define

\[
h = \frac{H}{2}.
\]

For a modeled timestamp \(\tau\), the implementation shrinks the half-window near the stroke boundaries:

\[
h(\tau)
=
\min\!\bigl(
h,\ \tau-t_{\min},\ t_{\max}-\tau
\bigr),
\]

where \(t_{\min}\) and \(t_{\max}\) are the first and last raw timestamps currently available.

The averaging interval is

\[
I(\tau) = [\tau-h(\tau),\ \tau+h(\tau)].
\]

If \(\tilde p(t)\) is the linearly interpolated raw path, the modeled position is

\[
x(\tau)
=
\frac{1}{|I(\tau)|}
\int_{I(\tau)} \tilde p(t)\,dt.
\]

The same method is applied to pressure and tilt.

The implementation evaluates this by integrating linearly interpolated segments with the **trapezoidal rule**.

---

## 4.3 Orientation averaging via circular mean

Averaging orientation angles directly is invalid around wraparound. The implementation instead averages unit vectors.

If \(\tilde \phi(t)\) is the interpolated orientation, then the modeled orientation is

\[
\phi(\tau)
=
\arg\!\left(
\frac{1}{|I(\tau)|}
\int_{I(\tau)}
\begin{bmatrix}
\cos\tilde\phi(t)\\[4pt]
\sin\tilde\phi(t)
\end{bmatrix}
dt
\right).
\]

This is the correct circular mean for directional quantities.

---

## 4.4 Position epsilon suppression

Let \(\varepsilon_p\) be the minimum modeled position spacing.

If the next candidate modeled position is too close to the previous one, it is discarded:

\[
\|x_k - x_{k-1}\| \le \varepsilon_p
\quad\Longrightarrow\quad
x_k\ \text{is suppressed}.
\]

This both reduces redundancy and stabilizes the modeled path.

---

## 4.5 Traveled distance

The cumulative traveled distance is

\[
d_0 = 0,
\qquad
d_k = d_{k-1} + \|x_k - x_{k-1}\|.
\]

This value is later reused by brush behaviors and by the tip modeler.

---

## 4.6 Velocity and acceleration

Once modeled positions are available, the implementation computes derivatives using another sliding-window procedure.

Abstractly, if \(t_-(k)\) and \(t_+(k)\) are the time bounds of the derivative window around \(\tau_k\), then

\[
v_k
=
\frac{x(t_+(k)) - x(t_-(k))}{t_+(k)-t_-(k)},
\]

and then

\[
a_k
=
\frac{v(t_+(k)) - v(t_-(k))}{t_+(k)-t_-(k)}.
\]

The code interpolates positions/velocities at the window boundaries when needed.

---

## 4.7 Stability criterion

Let \(\tau_{\mathrm{last\,real}}\) denote the timestamp of the last real input.

A modeled input becomes stable once future real inputs can no longer affect its symmetric averaging window:

\[
\tau_k + h < \tau_{\mathrm{last\,real}}.
\]

This criterion explains the stable/unstable split mathematically.

---

## 5. Algorithm 2: Brush Behavior Evaluation

The brush behavior engine is a small procedural program over normalized values.

## 5.1 Source normalization

Suppose a behavior source returns a scalar \(s\) and is configured with a value range \([r_0,r_1]\). The normalized parameter is

\[
u = \frac{s-r_0}{r_1-r_0}.
\]

Then one of the following out-of-range handlers is applied.

### Clamp

\[
B_{\mathrm{clamp}}(u)=\min(1,\max(0,u)).
\]

### Repeat

\[
B_{\mathrm{repeat}}(u)=u-\lfloor u\rfloor.
\]

### Mirror

\[
B_{\mathrm{mirror}}(u)
=
\left|u - 2\operatorname{round}\!\left(\frac{u}{2}\right)\right|.
\]

The final source-node output is

\[
\hat u = B(u)\in[0,1].
\]

If the source is indeterminate, the output is null.

---

## 5.2 Noise node

The noise generator is a 1D smooth random function driven by a stable linear congruential PRNG.

Let \(r_0, r_1 \in [0,1]\) be adjacent lattice values and let \(p\in[0,1)\) be the local phase. The output is

\[
N(p)
=
\operatorname{lerp}\bigl(r_0,r_1,\sigma(p)\bigr),
\]

where

\[
\sigma(p)=p^2(3-2p)
\]

is the standard smoothstep function.

So the noise is continuous and has continuous first derivative.

---

## 5.3 Damping node

The damping node implements exponential relaxation.

Let
- \(u_k\): current input,
- \(y_{k-1}\): previous damped value,
- \(\Delta_k\): progress increment,
- \(\gamma\): damping gap.

Then the update is

\[
y_k
=
u_k + (y_{k-1}-u_k)e^{-\Delta_k/\gamma}.
\]

This may be driven by:

- elapsed time,
- traveled distance in brush-size units,
- traveled distance in physical units.

---

## 5.4 Integral node

Let \(u_k\) denote the current node input and \(\Delta_k\) the progress increment. The integral node accumulates

\[
I_k = I_{k-1} + \frac{u_{k-1}+u_k}{2}\Delta_k.
\]

This is the trapezoidal rule.

The accumulated integral is then normalized by a configured range \([a,b]\) and passed through the out-of-range handler:

\[
\hat I_k
=
B\!\left(
\frac{I_k-a}{b-a}
\right).
\]

---

## 5.5 Binary operations

The binary operator node supports:

\[
ab,\quad a+b,\quad \min(a,b),\quad \max(a,b),
\]

and null-aware control-like operators such as:

- `andThen`,
- `orElse`,
- `xorElse`.

These allow the behavior graph to branch and combine signals.

---

## 5.6 Interpolation node

Two main interpolation forms appear:

### Linear interpolation

\[
\operatorname{lerp}(b,c;a)=b+a(c-b).
\]

### Inverse linear interpolation

\[
\operatorname{invlerp}(b,c;x)=\frac{x-b}{c-b},
\qquad b\ne c.
\]

---

## 5.7 Target mapping

If a target node receives normalized value \(u\in[0,1]\) and modifier range \([m_0,m_1]\), it emits

\[
m = m_0 + u(m_1-m_0).
\]

These modifiers are then aggregated per target:

- multiplicatively for targets like size, opacity, saturation,
- additively for targets like slant, rotation, pinch, rounding, offsets, hue, luminosity.

---

## 6. Algorithm 3: Brush-Tip State Creation

Let \(b\) denote the brush size and let the base brush-tip scales be \((s_x,s_y)\). Then the unmodified tip dimensions are

\[
w_0 = b s_x,
\qquad
h_0 = b s_y.
\]

The base brush-tip state also includes:

- rotation \(\alpha_0\),
- slant \(\sigma_0\),
- pinch \(\beta_0\),
- rounding \(\rho_0\).

After behavior evaluation, modifiers are applied.

A useful abstraction is:

\[
c_k = x_k + \Delta c_k,
\]

\[
w_k = \max(0,\ w_0 m_w),
\qquad
h_k = \max(0,\ h_0 m_h),
\]

\[
\sigma_k
=
\operatorname{clamp}\!\left(
\sigma_0 + \Delta \sigma_k,\ -\frac{\pi}{2},\ \frac{\pi}{2}
\right),
\]

\[
\beta_k = \operatorname{clamp}(\beta_0+\Delta\beta_k,\ 0,\ 1),
\]

\[
\rho_k = \operatorname{clamp}(\rho_0+\Delta\rho_k,\ 0,\ 1),
\]

\[
\alpha_k = \operatorname{wrap}_{(-\pi,\pi]}(\alpha_0+\Delta\alpha_k).
\]

Color/opacity/texture modifiers are applied similarly, with clamping or wrapping depending on the target.

---

## 7. Algorithm 4: Tip Shape Construction

The tip state is converted into a geometric object represented as the convex hull of a small set of circles:

\[
Q_k
=
\operatorname{conv}\!\left(\bigcup_{j=1}^{m_k} C_{k,j}\right),
\qquad 1\le m_k \le 4.
\]

This representation is rich enough to express rounded rectangles, slanted or pinched tips, and other dynamically changing brush footprints.

The shape module provides dedicated routines for generating polyline samples for:

- start caps,
- end caps,
- turns,
- whole isolated shapes.

Arc approximation is controlled by a **maximum chord height** tolerance.

---

## 8. Algorithm 5: Tangent-Constrained Extrusion

Given two consecutive tip shapes \(Q_{k-1}\) and \(Q_k\), the extruder wants a tangent join that smoothly connects them into a swept region.

However, this can fail if:

- one shape contains the other,
- the tangent connection does not cover the input shapes properly.

The implementation explicitly classifies tangent quality.

If the proposed extrusion is invalid, the code searches for an intermediate extrusion of the form

\[
E(\lambda)
=
(1-\lambda)E_{k-1} + \lambda E_k,
\qquad \lambda\in[0,1],
\]

such that the resulting tangent construction is valid.

If such a shape cannot be found, the implementation may:

- insert a break,
- reject the proposed extrusion,
- or split the geometry.

This is a geometric regularization step designed to reduce bad self-intersections and invalid winding.

---

## 9. Algorithm 6: Incremental Triangulation and Self-Intersection Handling

The geometry builder keeps two polyline sides:

- left side \(L=(\ell_0,\ell_1,\dots)\),
- right side \(R=(r_0,r_1,\dots)\).

As new vertices are appended, the builder attempts to connect them with triangles such as

\[
(\ell_i, r_i, \ell_{i+1})
\quad\text{or}\quad
(\ell_{i+1}, r_i, r_{i+1}),
\]

subject to winding and validity constraints.

The implementation distinguishes:

- a fast path for well-behaved ribbon growth,
- a slow path for degenerate or self-intersecting cases.

It includes explicit machinery to:

- detect self-intersections,
- reposition outline vertices onto intersection points,
- retriangulate portions of the mesh,
- undo prior retriangulation if later evidence shows it was wrong,
- insert extrusion breaks and start new partitions.

This is why the library can robustly handle loops, folds, and local jitter.

---

## 10. Algorithm 7: Static R-Tree for Mesh Queries

Once the stroke is packaged as a read-only `PartitionedMesh`, it supports fast geometric queries.

The acceleration structure is a **static R-tree** built using the **Sort-Tile-Recursive (STR)** algorithm.

Let:
- \(N\): number of leaves,
- \(B\): branching factor.

Then the bulk loader computes

\[
n_{\mathrm{tiles}}
=
\left\lceil \frac{N}{B} \right\rceil,
\qquad
s
=
\left\lceil \sqrt{n_{\mathrm{tiles}}} \right\rceil B.
\]

Then it:

1. sorts elements by \(x\),
2. splits them into vertical slices of size about \(s\),
3. sorts each slice by \(y\),
4. groups elements into tiles of size at most \(B\),
5. builds one parent bounding box per tile.

This yields a balanced hierarchy for intersection and coverage queries.

---

## 11. Algorithm 8: Polygon Tessellation

The geometry module also includes a generic tessellator.

Given a polyline \(P\), the function constructs a mesh for the interior defined by the **non-zero winding rule**.

If the polyline is self-intersecting, new vertices may be created at intersections.

If a tessellated output vertex already corresponds to an input vertex, its attributes are reused directly. Otherwise, its non-position attributes are interpolated from neighboring vertices:

\[
a(v_{\mathrm{out}})
\approx
\frac{\sum_j w_j a(v_j)}{\sum_j w_j}.
\]

The tessellation itself is implemented using `libtess2`.

---

## 12. Algorithm 9: Convex Hull

The repository also provides a generic convex hull routine.

For large point sets, it first applies the **Akl–Toussaint heuristic** to prune interior points.

Then it performs a Graham-scan-style hull construction:

1. choose the point with minimum \(y\) (breaking ties by minimum \(x\)),
2. sort points by polar angle about that start point,
3. while the last turn is not a strict left turn, pop the middle point.

The determinant test for removing concavities is

\[
\det\!\bigl(h_m-h_{m-1},\ p-h_m\bigr)\le 0.
\]

---

## 13. Conceptual Factorization of the Whole System

A concise mathematical description of the repo is:

\[
\mathcal{F}:
(\text{raw input stream},\ \text{brush program})
\mapsto
(\text{mesh},\ \text{outlines}).
\]

It factors into

\[
\mathcal{F}
=
\mathcal{E}
\circ
\mathcal{S}
\circ
\mathcal{T}
\circ
\mathcal{B}
\circ
\mathcal{M},
\]

where:

- \(\mathcal{M}\): input modeling,
- \(\mathcal{B}\): behavior evaluation,
- \(\mathcal{T}\): tip-state generation,
- \(\mathcal{S}\): analytic tip-shape construction,
- \(\mathcal{E}\): tangent-constrained extrusion and triangulation.

This is the cleanest abstraction of the codebase.

---

## 14. Main Design Insight

The repository is not just “path smoothing” and not just “mesh rendering.” Its main technical contribution is the combination of:

1. **incremental signal modeling**,
2. **procedural brush behavior evaluation**,
3. **analytic geometric tip representation**,
4. **robust tangent-based extrusion**,
5. **rollback-aware mesh updates**.

The central stable/unstable and fixed/volatile distinctions are what make the entire system interactive.

---

## 15. Short Executive Summary

If one wants a one-line description of the repository:

> `google/ink` implements a real-time operator that converts noisy, partially predicted pen input into a procedurally modulated sequence of analytic brush-tip shapes, then incrementally extrudes those shapes into a self-intersection-aware triangle mesh.

That is the right mental model for the codebase.
