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
- support `hug`, `fill`, numeric and percentage sizes, `min-w`/`max-w`/
  `min-h`/`max-h`, padding, gap, `gap="auto"`, basic align, `align="stretch"`,
  absolute children, lines, and text wrapping
- measure text with real font metrics during layout, so wrapped text reserves
  the height it is painted at
- build a paintable scene tree
- render fills, rounded rectangles, ellipses, borders, sided borders, dividers,
  SVG images, text, truncation, and simple text alignment
- draw rich text: `<segment>` runs with their own weight, style, size, and
  colour, wrapping as one continuous string
- resolve Google fonts through the renderer cache, and system fonts from the
  host's font directories
- apply variable font `wght` for text measurement and outline painting
- preserve bundled assets from `.gui` packages
- render raster images (PNG/JPEG) with `contain`, `cover`, `fill`, and `crop`
  fit modes
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

## Effects

`<appearance>` carries an ordered effect stack (RFC-0027), drawn in document
order:

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
| `layer-blur` | not implemented; reported rather than dropped silently |

`radius` is a blur radius as in CSS, which is twice the Gaussian sigma, and
`opacity` multiplies into the colour's alpha. `visible="false"` keeps an effect
in the document without drawing it.

Blur is three successive box blurs, the approximation the SVG filter spec
prescribes and browsers use.

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

## What The Tests Cover

Three layers, deliberately split by what each one can promise:

| Layer | Covers | Runs in CI |
|---|---|---|
| Unit tests | line breaking, font selection, grid translation, premultiplied alpha, per-segment colour | yes |
| Layout snapshots | the box tree of every example — position and size of every node | yes |
| Golden images | full-page appearance | no, by design |

The golden images are a local tool. They cannot be made reliable on a build
machine: two of the examples declare `source="system"` fonts, whose bytes
differ per host and cannot be redistributed, and the other two resolve Google
fonts and remote icons over the network, which is rate limited. Both failure
modes have happened. Making them hermetic would mean vendoring third-party
fonts and icons into this repository, which is not worth it — the layout
snapshots already catch geometry regressions on every platform, and the unit
tests cover painting behaviour. What the goldens add is a picture to look at,
which is worth having and not worth a red build.

So a green build means: nothing moved, and painting still behaves. It does not
mean anyone looked. Run the goldens by hand when changing anything visual.

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
- Add list marker support.
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

- Improve antialiasing quality for text and vector shapes.
- Add opacity groups and blend modes.
- Add layer blur.
- Add masks and clipping paths.
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
