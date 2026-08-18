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
- compute an early layout tree for rows, columns, stacks, frames, groups, and
  leaves
- support `hug`, `fill`, numeric sizes, padding, gap, `gap="auto"`, basic align,
  `align="stretch"`, absolute children, lines, and text wrapping
- build a paintable scene tree
- render fills, rounded rectangles, ellipses, borders, sided borders, dividers,
  SVG images, text, truncation, and simple text alignment
- resolve Google fonts through the renderer cache
- apply variable font `wght` for text measurement and outline painting
- preserve bundled assets from `.gui` packages
- expose a small WASM wrapper around the parser

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

The renderer resolves declared Google font faces through `.gui-render/cache`.
Text only uses declared family, weight, and style combinations. If a requested
face was not declared, the renderer falls back.

## Cache

The renderer cache lives at:

```text
.gui-render/cache
```

It is ignored by git. It can be deleted at any time; the renderer will recreate
it as needed.

## Backlog

### Preview And Developer Experience

- Build a real `.gui` preview command or viewer instead of only PNG export.
- Add a comparison workflow against HTML renderer output.
- Add golden-image tests for stable examples.
- Add clearer diagnostics for missing assets, unsupported tags, and failed font
  resolution.

### Layout Fidelity

- Continue matching the HTML renderer's row/col/stack behavior.
- Improve fill/hug behavior in nested mixed-axis layouts.
- Add support for min/max width and height.
- Add grid layout support.
- Improve frame/group absolute positioning semantics.
- Add clipping support during painting, not just scene capture.

### Text Fidelity

- Improve line-height, baseline, ascender/descender, and leading behavior.
- Improve wrapping to match the HTML renderer and Figma-exported widths.
- Add letter spacing support.
- Add `max-lines`, `truncate`, and ellipsis behavior for multi-line cases.
- Add richer text and list marker support.
- Add better text shaping for complex scripts and ligatures.

### Fonts

- Expand Google Fonts resolver beyond the current repository metadata path.
- Resolve static faces where available and variable faces where needed.
- Support italic variable axes where available.
- Add system font discovery.
- Add custom packaged fonts.

### Assets

- Add raster image support beyond SVG.
- Add image fit modes: contain, cover, fill, crop, none.
- Support package-relative asset paths consistently.
- Add asset cache metadata, expiration, size limits, and cleanup commands.

### Painting

- Improve antialiasing quality for text and vector shapes.
- Add shadows, opacity groups, blend modes, and effects.
- Add proper stroke alignment for inside, center, and outside strokes.
- Add masks and clipping paths.
- Add PDF export once the scene model is stable.

### WASM And API

- Expose layout, scene, and PNG rendering through WASM.
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
