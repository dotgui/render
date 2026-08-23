# dotgui render

Native Rust renderer for `.gui` files.

Repository: https://github.com/dotgui/render

## Why This Exists

`.gui` is intended to be a standalone interface format, closer in spirit to SVG
than to an app-specific export file. A `.gui` file should be something a tool can
open, inspect, preview, render, export, diff, cache, and eventually display live.

Today, the most complete renderer is the HTML renderer in dotgui kit. That is a
great reference implementation because browsers already solve layout, text,
fonts, SVGs, clipping, and repainting. The native renderer exists so `.gui` can
also be rendered without depending on a browser DOM.

The long-term goal is not just "export `.gui` to PNG." PNG export is the first
test harness. The real goal is a reusable rendering engine:

```text
.gui package
  -> parse
  -> resolve fonts/assets
  -> compute layout
  -> build scene
  -> paint to pixels
  -> show in a viewer, generate thumbnails, export PNG/PDF, or run through WASM
```

That lets `.gui` become visible anywhere: a desktop previewer, a CLI, a design
tool, a server-side thumbnail service, or a WebAssembly host.

## Design Principles

- The `.gui` file describes intent; the renderer resolves the practical details.
- Remote URLs are fetched and rendered as remote assets.
- Packaged local assets are resolved from inside the `.gui` package.
- Missing local assets are shown as broken assets, not silently guessed.
- Font declarations are strict: family, weight, and style must be declared.
- Renderer caches belong in `.gui-render/cache`; source packages are not mutated.
- The HTML renderer remains the behavioral reference while this native renderer
  catches up.

## Current Status

This is an early native renderer. It can already:

- read `.gui` packages and `.guix` XML
- extract `design.guix` and bundled package assets
- parse tokens, fonts, text styles, nodes, attributes, and children
- compute a flexbox layout tree (Taffy) for rows, columns, stacks, frames,
  groups, and leaves
- support `hug`, `fill`, numeric and percentage sizes, `min-width`/`max-width`/
  `min-height`/`max-height` (with `min-w`/`max-w`/`min-h`/`max-h` accepted as
  aliases), padding, gap, `gap="auto"`, basic align, `align="stretch"`,
  absolute children, lines, and text wrapping
- measure text with real font metrics during layout, so wrapped text reserves
  the height it is painted at
- build a paintable scene tree
- render fills, rounded rectangles, ellipses, borders, sided borders, dividers,
  SVG images, text, truncation, and simple text alignment
- paint the ordered `<fill>`, `<border>` and `<effect>` stacks of `<appearance>`,
  including linear, radial and conic gradients and image fills
- draw outlines with `outline`/`outline-offset`, squircle corners with
  `corner-smoothing`, and the `shadow` shorthand
- clip per axis with `overflow-x`/`overflow-y`, or to the node's shape with
  `clip`
- composite with `blend`, `isolation`, `filter` and `z-index`, and reuse named
  effect stacks with `effect-style` and colours with `fill-style`
- mask with `clip-path`, an image `mask-src`, or a `mask` child
- transform with `rotation`, `scale-x`/`scale-y`, `skew-x`/`skew-y`, `flip` and
  `transform-origin`, and shape boxes with `aspect-ratio`
- drive variable axes from `font-stretch` and `font-optical-sizing`, and place
  and resample images with `object-position` and `image-rendering`
- break lines by `white-space`, `text-wrap` and `word-break`, and space them
  with `word-spacing`, `paragraph-indent` and `paragraph-spacing`
- draw list markers with `list`/`list-level`/`list-marker`, and place text with
  `vertical-align`, `leading-trim` and `baseline-shift`
- draw rich text: `<segment>` runs with their own weight, style, size, and
  colour, wrapping as one continuous string
- resolve Google fonts through the renderer cache, and system fonts from the
  host's font directories
- apply variable font `wght` for text measurement and outline painting
- preserve bundled assets from `.gui` packages
- render raster images (PNG/JPEG) with `contain`, `cover`, `fill`, and `crop`
  fit modes
- expand `<instance>` nodes against `<components>` definitions, with declared
  props, ad-hoc overrides by layer id, variants, and instance scaling
- expose parsing, layout, scene, and PNG rendering through WASM

It is not complete yet. The renderer should be treated as a growing engine, not
as a pixel-perfect replacement for the HTML renderer.

## Project Layout

```text
crates/
  renderer/       Core Rust parser, model, layout, scene, assets, fonts, paint
  renderer-wasm/  JavaScript/WASM adapter around the core parser

examples/         Sample .gui packages used for renderer validation
out/              Local PNG exports, ignored by git
.gui-render/      Local renderer cache, ignored by git
```

## Commands

Run tests:

```bash
cargo test
```

Layout snapshots record the box tree for every example as plain text. They
measure text by character count rather than font metrics, so they produce the
same result on any machine and are the regression signal CI relies on. After an
intentional layout change, read the diff and regenerate:

```bash
UPDATE_SNAPSHOTS=1 cargo test -p dotgui-renderer --test layout_snapshots
```

Golden-image tests compare four examples against committed PNGs. They are a
local tool rather than a CI gate, and are ignored by default: two of them
declare `source="system"` fonts whose bytes differ per host, and the other two
resolve Google fonts and remote icons over the network, which is rate limited.
Each golden records the fingerprints of the fonts it was made with in a
`.fonts` file, so a mismatch reports itself instead of showing an unexplained
pixel diff.

```bash
cargo test -p dotgui-renderer --test golden_tests -- --ignored
```

After an intentional rendering change, regenerate and eyeball them:

```bash
UPDATE_GOLDENS=1 cargo test -p dotgui-renderer --test golden_tests -- --ignored
```

Build the browser bundle. The `net` feature is off for wasm32 (its TLS stack
does not cross-compile), so assets and fonts have to travel inside the `.gui`
package:

```bash
cargo build -p dotgui-renderer-wasm --target wasm32-unknown-unknown
```

Render any `.gui` file to PNG:

```bash
cargo run -q -p dotgui-renderer --example render_png <input.gui> <output.png>
```

Example:

```bash
cargo run -q -p dotgui-renderer --example render_png examples/beacon-recovery-code-android.gui out/beacon.png
```

Parse a `.gui` or `.guix` file and print JSON:

```bash
cargo run -q -p dotgui-renderer --example parse examples/beacon-recovery-code-android.gui
```

Print the layout tree:

```bash
cargo run -q -p dotgui-renderer --example layout examples/beacon-recovery-code-android.gui
```

Print the paintable scene tree:

```bash
cargo run -q -p dotgui-renderer --example scene examples/beacon-recovery-code-android.gui
```

Inventory tags and attributes across examples:

```bash
cargo run -q -p dotgui-renderer --example inventory examples
```

Compare this renderer against the kit HTML renderer:

```bash
cargo run -q -p dotgui-renderer --example compare                    # whole corpus
cargo run -q -p dotgui-renderer --example compare examples/foo.gui   # one document
```

Needs [bun](https://bun.sh), a Chromium-based browser, and a `dotgui/kit`
checkout beside this one (or `DOTGUI_KIT` pointing at it) with its render
bundle built. See [Comparing Against Kit](#comparing-against-kit).

## Appearance

`<appearance>` carries a node's complete paint: ordered stacks of `<fill>`,
`<border>` and `<effect>`, each drawn in document order, so the last entry ends
up on top.

```xml
<col w="fill" radius="16">
  <appearance>
    <fill type="color" value="$card" />
    <border color="$outline-variant" w="1" align="inside" />
    <effect type="drop-shadow" x="0" y="2" radius="6" color="#0000001F" />
  </appearance>
</col>
```

A `<fill>` carries `type` and `value`; `type="color"` is painted, and gradient
and image fills are carried into the scene but not painted yet. A `<border>`
carries `color`, `w`, `align` and `style`; `w` takes one to four numbers, like
the `border` shorthand.

A `<fill>` value may be a colour, a gradient or an image. Gradients follow CSS
— `linear-gradient()`, `radial-gradient()` and `conic-gradient()`, with
`angular-gradient()` accepted as the spec's name for the last — and angles
follow CSS too, so `0deg` points up and they turn clockwise. Stops that declare
no position are spaced evenly. An image fill is drawn with its `fit` mode and
held inside the node's own outline, so a rounded or elliptical node clips it.

The same values work in the `fill` attribute, since it is the same fill with
one entry: `fill="linear-gradient(135deg, #FF6B6B 0%, #4ECDC4 100%)"`.

When `<appearance>` declares no fill, the node's `fill` attribute is used
instead; the same goes for `<effect>` and the `shadow` shorthand. When it
declares at least one `<border>`, the node's `border` shorthand is ignored, as
the spec requires. `visible="false"` keeps an entry in the document without
drawing it.

### Outlines

`outline` takes the same value as `border` — width, colour, style — but is
drawn outside the box and takes no part in layout, so it can overlap its
neighbours. `outline-offset` pushes it further out:

```xml
<rect w="40" h="40" radius="8" fill="$surface" outline="2 $focus" outline-offset="3" />
```

An outline follows the node's corners, growing with the box, so a square box
keeps a square outline. Outlines are uniform: a sided value collapses to its
widest side rather than drawing four strokes.

### Corner Smoothing

`corner-smoothing` is the squircle control, `0` to `1` (or `0%` to `100%`). It
does not change how close the corner comes to the box corner; it spreads the
same turn over a longer run of each edge, reaching
`radius * (1 + corner-smoothing)`, which is what removes the curvature jump
where the curve meets the straight edge.

Each corner is one cubic, an approximation of Figma's construction rather than
a reproduction of it. When the reach does not fit — a large radius at high
smoothing — the radius gives way, not the smoothness.

### Overflow

`clip` clips a node's children to its own shape, rounded corners included.
`overflow-x` and `overflow-y` clip one axis each, and either overrides `clip`
on its own axis, as in CSS:

```xml
<col w="200" h="120" clip overflow-y="visible">
```

Of the four spec values only `visible` shows what escapes the box. `scroll` and
`auto` clip as well — a still frame has no scrollbar to drag, so what they
reveal is the same first screenful `hidden` does.

One axis on its own clips to a band across the canvas rather than to the node's
shape: the unclipped direction has no edge to stop at, and a rounded corner
needs both edges to curve between.

### Shadows

`shadow` is the single-shadow shorthand, with CSS `box-shadow` values:
`x y blur [spread] color`, and `inset` for an inner shadow. It is read as one
entry of the effect stack, so a node that declares `<effect>` children ignores
it.

### Compositing

`blend` composites a node against what is already painted behind it, with CSS's
`mix-blend-mode` names. `isolation` makes a node its own stacking context, so a
descendant's blend mode sees only that subtree. `filter` takes a CSS filter
list. `z-index` decides paint order among siblings:

```xml
<rect w="80" h="60" fill="$accent" blend="multiply" />
<col w="fill" isolation filter="grayscale(1) brightness(1.2)" />
<rect w="80" h="60" fill="$brand" z-index="1" />
```

`opacity` is a group property, as in CSS: the subtree is drawn solid and the
whole thing is faded once. Overlapping children inside a translucent group do
not compound, and a group's shadow fades with it.

A node with an opacity, a blend mode, a filter or `isolation` is painted onto a
layer of its own and composited in one go; everything else paints straight onto
the canvas. Backdrop effects read what is behind a node, and inside its own layer
that is nothing, so a node that both isolates and blurs its backdrop is a known
gap.

`filter` implements `blur`, `brightness`, `contrast`, `grayscale`, `invert`,
`opacity`, `saturate` and `sepia`, with the colour matrices the Filter Effects
spec defines. Functions apply left to right. Anything else in the list is
skipped, so an unimplemented function leaves the node unfiltered rather than
blank.

`z-index` is resolved when the scene is built, not when it is painted: a node
without one sorts as 0 and the sort is stable, so document order still decides
between equals.

### Line Breaking

`white-space` and `text-wrap` decide whether text wraps at all; either one
saying `nowrap` is enough, and `white-space: pre` says it too. Turning wrapping
off stops the renderer choosing breaks — it does not overrule a newline the
document wrote.

`word-break` decides where a break may fall:

| Value | Behaviour |
|---|---|
| `normal` | between words, and after `-`, `–`, `—`, `/` |
| `break-all` | between any two characters |
| `keep-all` | between words only, not at punctuation |
| `break-word` | as `normal`, but a word alone on a line and still too long is split |

`word-spacing` adds pixels to every space, in measurement as well as painting.
`paragraph-indent` takes room from the first line only, so the rest of the
block is unaffected. `paragraph-spacing` is room after the block, which it gets
from a bottom margin, as kit does.

`thickness` gives a `<line>` its height. A `<line>` is a divider with no height
of its own, and the spec's default of 1px is what it drew before.

### Lists And Vertical Metrics

`list` marks a `<text>` node as a list item and draws a marker before its first
line: a bullet for `disc`, a number for `decimal`, or whatever `list-marker`
says. `list-level` indents the whole block, one 16px step per level.

A decimal item is numbered by its place among its **list-item siblings**, so a
plain `<text>` in between does not take a number — as in CSS, where only a
`display: list-item` box advances the counter. `disc` items advance it too;
they simply do not show it.

`vertical-align` places a block shorter than its box: `top` (the default),
`center` or `bottom`.

`leading-trim="cap-height"` takes the half-leading off the top of the block, so
a capital's top edge sits on the box's top edge instead of half a line below
it. Layout and painting call the same `leading_trim` — one number rather than
an ascender and a cap height each side combines for itself — so a trimmed box
is sized and drawn to the same edge.

`baseline-shift` lifts one run off the line's shared baseline without moving
anything else on it, which is how a superscript is written. It is not
inherited: a nested run would otherwise double its parent's shift.

### Fonts And Images

`font-stretch` drives a variable face's `wdth` axis, the way `font-weight`
already drives `wght`. CSS's keywords are exact percentages — `condensed` is
75%, `expanded` is 125% — so a keyword and its percentage are the same request.

`font-optical-sizing` drives the `opsz` axis from the font size. It defaults
to `auto`, as in CSS, so a face carrying the axis is optically sized unless a
document says `"none"`.

Only variable faces with the axis are affected, which is narrower than it
sounds: every Google family in the corpus resolves to a static instance with no
axes at all, so this reaches the `SF Pro` system fonts and nothing else. It is
still worth having right — turning it on is what closed `harbor`'s 17px
disagreement with kit, since a browser has been optically sizing that text all
along.

`font-smoothing="none"` draws glyphs without antialiasing. `antialiased` and
`subpixel-antialiased` are both the grayscale antialiasing already used;
subpixel rendering needs the target display's own stripe order, which a PNG has
no business assuming.

`image-rendering` picks the resampling filter: `pixelated` and `crisp-edges`
keep hard pixel edges, and anything else smooths. `object-position` decides
where an image sits in whatever room its `fit` mode leaves over — it has no
effect under `fit="fill"`, which stretches to every edge.

`href` has nothing to paint in a still frame, so it is carried into the scene
for an interactive consumer rather than dropped.

### Transforms

`rotation`, `scale-x`, `scale-y`, `skew-x`, `skew-y` and `flip` compose into
one matrix per node, pivoted on `transform-origin`:

```xml
<rect w="80" h="30" fill="$accent" rotation="30" />
<img src="assets/photo.webp" w="120" h="80" flip="h" scale-y="1.2" />
<col w="fill" rotation="45" transform-origin="top-left" />
```

The parts apply in CSS's order — rotate, then flip and the scales, then the
skews — and each is a further multiplication, so `skew-x` and `skew-y` compose
as two matrices rather than one, which is what keeps the cross term CSS
produces. `flip` is a mirror, so it folds into the scales as a factor of -1 and
multiplies with an explicit `scale-x` rather than replacing it.

`transform-origin` takes the spec's hyphenated keywords (`top-left`,
`middle-right`), CSS's own (`left`, `bottom`), percentages and lengths. It
defaults to the centre, as CSS does when a transform is present.

A transformed node is painted onto its own layer and the matrix is applied when
that layer is composited — the same layer blend modes, filters and masks use.
That is one matrix for a whole subtree rather than one threaded through every
draw, and the price is resampling: a transformed node is slightly softer than
drawing its geometry transformed would be.

`aspect-ratio` is layout, not paint. It takes `16/9` or a bare number and is
handed to Taffy, so it shapes whichever axis the layout leaves free — across a
column's cross axis the stretch still wins.

### Masks And Clipping

Three things shape a node, and all three cut its own paint as well as its
children — unlike `clip`, which only holds children in:

```xml
<frame w="200" h="120" clip-path="inset(10px round 20px)" />

<group w="390" h="200" mask-src="assets/mask.svg" mask-x="0" mask-y="0"
       mask-width="390" mask-height="200" />

<group w="100" h="100">
  <ellipse w="100" h="100" mask="true" />
  <img src="assets/photo.webp" w="100" h="100" fit="cover" />
</group>
```

`clip-path` builds `inset()`, `circle()`, `ellipse()` and `polygon()` directly,
with percentages resolved against the node's box. A `path()` value is handed to
the SVG parser, which already knows that grammar. Anything else leaves the node
unclipped rather than blank.

`mask-src` draws its asset once at `mask-x`/`mask-y`, sized by
`mask-width`/`mask-height` or the node itself. `mask-mode` takes the source's
`alpha` (the default) or its `luminance`. `mask-composite` is `add` by default;
`subtract` and `exclude` cut the shape out instead — a single mask layer gives
CSS's operators nothing to combine against, and `mask-src` is hoisted off a
Figma group mask, where that is what they mean.

A child carrying `mask` shapes its parent with its own outline and is not
painted. It stays in the scene tree so a consumer can still see what the shape
was.

A mask that cannot be resolved leaves the node alone. Losing content over a
missing file would be worse than showing it unmasked.

### Effect Styles

`<styles>` also holds `<fill-style name="X" value="..." />`, which a node
picks up with `fill-style="X"`. A direct `fill` wins over the named style.

`<styles>` holds named `<effect-style>` blocks alongside `<text-style>` ones. A
node referencing one by name picks up its effects **under** its own, so a node
can add to a style rather than only replace it:

```xml
<styles>
  <effect-style name="card">
    <effect type="drop-shadow" x="0" y="2" radius="6" color="#0000001F" />
  </effect-style>
</styles>

<col w="fill" radius="12" fill="$card" effect-style="card" />
```

### Effects

The effect stack (RFC-0027) is drawn in document order:

```xml
<row w="fill" h="90" radius="16" fill="$surface">
  <appearance>
    <effect type="drop-shadow" x="0" y="2" radius="6" spread="0" color="#0000001F" />
    <effect type="drop-shadow" x="0" y="16" radius="32" spread="-8" color="#00000029" />
  </appearance>
</row>
```

| Type | Behaviour |
|---|---|
| `drop-shadow` | the node's outline, grown by `spread`, moved by `x`/`y`, blurred, drawn behind |
| `inner-shadow` | the same shape inverted and clipped to the node, drawn over the fill |
| `background-blur` | blurs whatever is already painted behind the node |
| `glass` | background blur plus a `saturation` percentage |
| `layer-blur` | blurs the node and everything in it, leaving the backdrop alone |

`radius` is a blur radius as in CSS, which is twice the Gaussian sigma, and
`opacity` multiplies into the colour's alpha.

`layer-blur` and `background-blur` are opposites worth keeping straight: one
softens the node and its children and leaves what is behind them sharp, the
other softens the backdrop and leaves the node sharp. A layer blur is applied
to the node's own layer, so it spreads past the node's box the way a blurred
thing should.

Blur is three successive box blurs, the approximation the SVG filter spec
prescribes and browsers use.

## Components

A `<components>` block declares reusable bodies; an `<instance>` names one and
passes overrides as attributes.

```xml
<components>
  <component name="Card/Product" id="comp-card">
    <props>
      <prop name="title" type="text" target="title" />
    </props>
    <col w="320" radius="12" fill="#fff" p="16" gap="8">
      <text id="title" value="Product Name" font-size="16" />
    </col>
  </component>
</components>

<instance component="comp-card" title="Nike Air Max 90" x="24" y="120" />
```

Instances are expanded **while the document is parsed**, so nothing downstream
ever sees one: layout, the scene and painting work on the tree the document
would have had if it were written out longhand.

A declared `<prop>` names its `target` layers by id, and `bind` says which
attribute a value lands on when the type alone does not. An instance attribute
with no `<prop>` behind it matches a layer id directly, taking its type from
what it finds there. `false` removes a layer. A `<component-set>`'s `<variant>`
children are components in their own right, referenced by their own ids.

An instance that declares a different `w`/`h` than its component body scales
the children that ask for it with `constraint-h`/`constraint-v`. That is the
only thing those constraints do — in this renderer and in kit — so a document
using them to pin an edge still gets nothing; see the tracking issue.

A component whose body instantiates itself stops at a depth limit rather than
expanding forever, and an instance naming a component nothing declares is
dropped rather than laid out as an unknown block.

## Positioned Containers

`<frame>` and `<group>` position their children: a child sits at its own
`x`/`y` relative to the container, and siblings overlap rather than stacking.
That is what a card overlay, a badge on a photo, or a knob on a slider track is
written as.

```xml
<frame w="328" h="200" radius="16" clip>
  <img src="assets/hero.webp" x="0" y="0" w="328" h="200" fit="cover" />
  <rect x="0" y="86" w="328" h="114" fill="linear-gradient(180deg, #0000, #000C)" />
  <text x="16" y="128" value="Wheel throwing" font-size="28" fill="#FFFFFF" />
</frame>
```

Only `<stack>`, `<row>`, `<col>` and `<grid>` lay their children out in flow,
where `abs` takes an individual child out of it. This matches kit, which
renders a `<frame>`'s children with `isAbsolute = !isStack`, and the spec,
which says a `<group>`'s "children are absolutely positioned relative to the
group origin".

## Grid

`<grid>` has three shapes, chosen by which attributes are present (RFC-0032).

**Track grid** — the parent declares track sizes:

```xml
<grid cols="200 1fr" rows="56 1fr" gap="0" w="fill" h="fill">
  <row gc="1/-1" gr="1" h="fill">…</row>   <!-- header, spans all columns -->
  <col gc="1" gr="2">…</col>               <!-- sidebar -->
  <col gc="2" gr="2">…</col>               <!-- content -->
</grid>
```

```text
cols="3"         →  three equal columns
cols="240 1fr"   →  240px then the remaining space
cols="auto 1fr"  →  content-sized then the rest
cols="fill 200"  →  as many >=200px columns as fit
```

A bare integer means different things by position: alone it is a track *count*,
inside a list it is a pixel *size*.

**Unit grid** — the parent declares a unit size, becoming a snapped coordinate
space of `w / unit` by `h / unit` squares. For freely placed and overlapping
elements; children at the same coordinates stack in document order.

```xml
<grid unit="8" w="320" h="400">
  <img gc="1/40" gr="1/14" fit="cover" src="$cover" />        <!-- fills the span -->
  <img gc="13" gr="9" w="128" h="128" radius="64" src="$me" /><!-- fixed px size -->
</grid>
```

**Auto flow** — `columns="N"` remains valid as the legacy spelling of `cols="N"`.

### Placement

`gc` and `gr` are grid-column and grid-row, named to avoid colliding with the
`<col>` and `<row>` tags. Both accept a line (`"3"`) or an inclusive range
(`"2/5"` is columns 2 through 5). Negative indices count from the end, so
`gc="1/-1"` spans every column. `col-span` and `row-span` span a count from the
current position, and `col-span="all"` reaches the last line.

`w` and `h` are always pixels, in every mode. A child fills its span when the
matching axis carries a range and no explicit size is given; an explicit `w` or
`h` always wins.

## Rich Text

A `<text>` node can be split into styled runs with `<segment>`:

```xml
<text font-family="Inter" font-size="16" fill="$text">
  <segment value="Total " />
  <segment value="$318.18" font-weight="700" fill="$accent" font-size="26" />
  <segment value=" due today" />
</text>
```

Each segment inherits every property it does not override, and accepts
`font-family`, `font-weight`, `font-style`, `font-size`, `line-height`,
`letter-spacing`, `fill`/`color`, and `text-style`.

Text flows continuously across segments, so wrapping, `max-lines`, `truncate`,
and alignment behave exactly as they do for a plain string — a line can break
in the middle of a segment, and the ellipsis lands wherever the text runs out.
Runs on a line share a baseline, and the line is as tall as its largest run.

Segments are content, not boxes: they never appear in the layout or scene tree
as nodes.

## Asset And Font Resolution

Remote assets:

```xml
<img src="https://api.iconify.design/material-symbols/menu-rounded.svg" w="24" h="24" />
```

The renderer fetches the URL, caches it, and renders it.

Packaged local assets:

```xml
<img src="assets/icon.svg" w="24" h="24" />
```

The renderer looks for `assets/icon.svg` inside the `.gui` package first.

Missing local assets:

```xml
<img src="assets/missing.svg" w="24" h="24" />
```

The renderer shows a broken asset marker. It does not guess a remote source.

Google fonts:

```xml
<fonts>
  <font family="Roboto" source="google" weights="400 500 700" styles="normal" />
</fonts>
```

Google families resolve through the Google Fonts CSS API — the same source the
HTML renderer uses — with one request per family covering every declared weight
and style, cached under `.gui-render/cache`.

`source="system"` families are looked up in the host's font directories. When
the declared family is not installed the renderer falls back to the platform UI
font, mirroring the CSS stack the HTML renderer emits, and records what it
substituted:

```text
warning: 'SF Pro Display' is not installed; rendered with 'DejaVu Sans'
```

Read those through `FontStore::warnings()`. A render that quietly used the wrong
typeface is worse than one that says so — which matters most when rendering
somewhere the declared fonts cannot exist, such as a Linux server asked for an
Apple system font.

Text only uses declared family, weight, and style combinations. If a requested
face was not declared, the renderer falls back.

## Cache

The renderer cache lives at:

```text
.gui-render/cache
```

It is ignored by git. It can be deleted at any time; the renderer will recreate
it as needed.

## Spec Coverage

[`COVERAGE.md`](COVERAGE.md) records which spec properties this renderer
implements, and which elements each one works on. It is generated, never
hand-edited, and a stale copy fails the build — so it cannot drift into
claiming more than is true.

It reads by property rather than by element because that is the unit of work:
implementing `radius` is one job across every element that allows it, not one
job per element. A property can also be *partial* — read on some elements and
not others — which is the cheapest kind of gap to close.

Coverage is *declared* in `crates/renderer/src/coverage.rs` rather than
inferred by scanning the sources for attribute names, which would count an
attribute as implemented because it appears in a comment. Implementing an
attribute means adding it to that list in the same change.

```bash
UPDATE_COVERAGE=1 cargo test -p dotgui-renderer --test spec_coverage
```

The spec itself is vendored at `spec/spec.json` rather than read from a
sibling `dotgui/core` checkout, so generation and the check work from a
checkout of this repository alone. Refresh it deliberately:

```bash
cargo run -p dotgui-renderer --example refresh_spec
```

A weekly job watches `dotgui/core` and opens an issue when the vendored copy
falls behind. That job is allowed to reach the network because its failure
means "the spec moved", not "the renderer is broken"; the build gate stays
offline.

## What The Tests Cover

Three layers, deliberately split by what each one can promise:

| Layer | Covers | Runs in CI |
|---|---|---|
| Unit tests | line breaking, font selection, grid translation, premultiplied alpha, per-segment colour | yes |
| Pixel tests | what painting actually puts on the canvas, asserted per pixel | yes |
| Layout snapshots | the box tree of every example and fixture — position and size of every node | yes |
| Golden images | full-page appearance | no, by design |
| Kit comparison | where the two renderers disagree, ranked | no, by design |

### Fixtures

The 27 example packages are real documents, and they exercise about half the
format. `crates/renderer/tests/fixtures` holds hand-written `.guix` documents
for the rest — gradients, masks, transforms, compositing, group opacity,
components, blur, and the text controls — so every feature has a page that
uses it.

They are not decoration. A feature with no document using it is a feature
nobody has looked at: writing the text-breaking fixture is what turned up that
`word-break="break-word"` never split a word that was alone on its line, which
the unit tests had missed by always putting another word in front of it.

Fixtures are picked up automatically by the layout snapshots, and the
paint-heavy ones have goldens.

The golden images are a local tool. They cannot be made reliable on a build
machine: two of the inputs declare `source="system"` fonts, whose bytes
differ per host and cannot be redistributed, and the other two resolve Google
fonts and remote icons over the network, which is rate limited. Both failure
modes have happened. Making them hermetic would mean vendoring third-party
fonts and icons into this repository, which is not worth it — the layout
snapshots already catch geometry regressions on every platform, and the unit
tests cover painting behaviour. What the goldens add is a picture to look at,
which is worth having and not worth a red build.

So a green build means: nothing moved, and painting still behaves. It does not
mean anyone looked. Run the goldens by hand when changing anything visual.

### Comparing Against Kit

`--example compare` renders every example and fixture twice — once here, once
through the kit HTML renderer in a headless Chromium — and ranks the documents
by how far the two disagree.

kit is the behavioural reference, but it is **not** the arbiter. It has
repeatedly disagreed with the spec, and either side can be the wrong one. The
output is a worklist, not a bug list: every entry still has to be adjudicated
against `spec/spec.json` by hand. The first run made that concrete — the
loudest result was `compositing.guix` at 103x62 against our 460x240, which is
kit failing to parse a bare `isolation` attribute, and the largest pixel
difference was a backdrop-blur panel where our value matched the source-over
arithmetic to a rounding step and kit's did not.

What it ranks on is **geometry**, not pixels. Two rasterisers never agree on
glyph pixels, so a raw pixel diff is drowned by antialiasing on every row of
text. Instead each row is reduced to a coarse signature, the two sequences are
diffed the way `diff` diffs lines, and what gets reported is the points where
the vertical alignment steps and *stays* stepped — a box with a different
height, with everything below it displaced. A row the diff cannot pair up is
not by itself interesting; an offset that holds for the rest of the page is.

Pixel difference over the rows that did line up is reported as a secondary
number. It never reaches zero on text, and is not meant to.

Documents whose fonts kit could not load are marked `[font]` and kept out of
the ranking. A substituted face has its own advance widths, so a string wraps
somewhere else and every box that hugs it is a different size — the numbers are
then about the font, not the layout. `harbor-report-post-ios` sat at the top of
the list at -21px until this was added: it declares `SF Pro Display`, which
headless Chromium does not have, so kit drew the whole document in `system-ui`
while this renderer used the real face.

Availability is measured rather than asked. `document.fonts.check()` reports
true whenever the stack has any usable fallback, and it called `SF Pro Display`
present on a machine that rendered every glyph in something else. The probe
instead requests a family with no fallback and compares it against a name that
cannot exist: equal widths mean both landed on the same substitute.

Like the goldens, this is a local tool and not a build gate. It shells out to a
separate repository, needs a browser, and most examples fetch their icons over
the network.

```bash
cargo run -q -p dotgui-renderer --example compare
```

#### Naming the element behind a divergence

The ranking says a document went out of alignment at some row. `--boxes` says
which element did it: both trees are walked in document order, paired up, and
every box whose size differs is listed with the ancestors it dragged along.

```bash
cargo run -q -p dotgui-renderer --example compare -- --boxes examples/foo.gui
```

This is the diagnostic that has found every layout divergence so far. The
`<line>` bug was three rows of it — `h 1 vs 0` in a document that was three
pixels short — and it named `beacon`'s remaining six pixels as one `<row>`
carrying `min-h`.

When the two trees have different shapes the boxes cannot be paired at all.
That is reported rather than worked around, because it means one renderer
built nodes the other did not, which is a bigger finding than any size.

## Backlog

### Preview And Developer Experience

- Build a real `.gui` preview command or viewer instead of only PNG export.
- Add a comparison workflow against HTML renderer output.
- Add clearer diagnostics for missing assets, unsupported tags, and failed font
  resolution.

### Layout Fidelity

- Continue matching the HTML renderer's row/col/stack behavior.
- Improve fill/hug behavior in nested mixed-axis layouts.
- Improve frame/group absolute positioning semantics.

### Text Fidelity

- Improve line-height, baseline, ascender/descender, and leading behavior.
- Improve wrapping to match the HTML renderer and Figma-exported widths.
- Add better text shaping for complex scripts and ligatures.

### Fonts

- Expand Google Fonts resolver beyond the current repository metadata path.
- Resolve static faces where available and variable faces where needed.
- Support italic variable axes where available.
- Add custom packaged fonts.

### Assets

- Support package-relative asset paths consistently.
- Add asset cache metadata, expiration, size limits, and cleanup commands.

### Painting

- Add `border-image`; the spec types it as a string but does not define the
  value grammar, so it needs pinning down against kit first.
- Improve antialiasing quality for text and vector shapes.
- Add PDF export once the scene model is stable.

### WASM And API

- Design a stable Rust API for host applications.
- Add incremental update APIs for live previews.
- Support repainting from an in-memory document model.

## First Commit Checklist

Before publishing the first commit:

```bash
cargo test
git status
git add .
git commit -m "Initial native dotgui renderer"
git push -u origin main
```

If your default branch is not `main`, replace `main` with the branch name.
