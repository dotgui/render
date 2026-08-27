# Spec coverage

Generated — do not edit by hand. Run `UPDATE_COVERAGE=1 cargo test -p dotgui-renderer --test spec_coverage` and commit the result.

Measured against `spec/spec.json`, vendored from `dotgui/core` at commit `bdcb1eb7b5f5`. Coverage is declared in `crates/renderer/src/coverage.rs`, not inferred from the sources, so a row says the renderer reads that property and acts on it.

Listed by property, because that is the unit of work: implementing `radius` is one job across every element that allows it, not one job per element.

| | Properties |
|---|---|
| Implemented | **109** |
| Partial | **1** |
| Not implemented | **10** |
| Total | **120** |

## Not implemented

The work list. Each row is one property to add, and the elements it has to work on.

| Property | Elements |
|---|---|
| `border-image` | `<frame>`, `<stack>`, `<row>`, `<col>`, `<grid>` |
| `constraint-h` | `<frame>`, `<stack>`, `<row>`, `<col>`, `<grid>`, `<group>`, `<text>`, `<img>`, `<rect>`, `<ellipse>`, `<line>` |
| `constraint-v` | `<frame>`, `<stack>`, `<row>`, `<col>`, `<grid>`, `<group>`, `<text>`, `<img>`, `<rect>`, `<ellipse>`, `<line>` |
| `fill-rule` | `<rect>`, `<ellipse>` |
| `font-feature` | `<text>` |
| `font-postscript` | `<text>` |
| `font-style-name` | `<text>` |
| `text-rendering` | `<text>` |
| `text-resize` | `<text>` |
| `writing-mode` | `<text>` |

## Partially implemented

Read on some elements but not others — usually cheaper to finish than to start.

| Property | Implemented on | Missing on |
|---|---|---|
| `direction` | `<stack>`, `<line>` | `<text>` |

## Implemented

| Property | Elements |
|---|---|
| `<border>` | `<appearance>` |
| `<effect>` | `<appearance>` |
| `<fill>` | `<appearance>` |
| `abs` | `<frame>`, `<stack>`, `<row>`, `<col>`, `<grid>`, `<group>`, `<text>`, `<img>`, `<rect>`, `<ellipse>`, `<line>` |
| `align` | `<stack>`, `<row>`, `<col>`, `<text>` |
| `aspect-ratio` | `<frame>`, `<stack>`, `<row>`, `<col>`, `<grid>`, `<group>`, `<text>`, `<img>`, `<rect>`, `<ellipse>`, `<line>` |
| `baseline-shift` | `<text>` |
| `blend` | `<frame>`, `<stack>`, `<row>`, `<col>`, `<grid>`, `<group>`, `<text>`, `<img>`, `<rect>`, `<ellipse>`, `<line>` |
| `border` | `<frame>`, `<stack>`, `<row>`, `<col>`, `<grid>`, `<img>`, `<rect>`, `<ellipse>` |
| `border-align` | `<frame>`, `<stack>`, `<row>`, `<col>`, `<grid>`, `<img>`, `<rect>`, `<ellipse>` |
| `border-color` | `<frame>`, `<stack>`, `<row>`, `<col>`, `<grid>`, `<img>`, `<rect>`, `<ellipse>` |
| `border-style` | `<frame>`, `<stack>`, `<row>`, `<col>`, `<grid>`, `<img>`, `<rect>`, `<ellipse>` |
| `border-width` | `<frame>`, `<stack>`, `<row>`, `<col>`, `<grid>`, `<img>`, `<rect>`, `<ellipse>` |
| `clip` | `<frame>`, `<stack>`, `<row>`, `<col>`, `<grid>` |
| `clip-path` | `<frame>`, `<stack>`, `<row>`, `<col>`, `<grid>` |
| `col-gap` | `<grid>` |
| `columns` | `<grid>` |
| `corner-smoothing` | `<frame>`, `<stack>`, `<row>`, `<col>`, `<grid>`, `<img>`, `<rect>` |
| `decoration` | `<text>` |
| `decoration-color` | `<text>` |
| `decoration-style` | `<text>` |
| `decoration-thickness` | `<text>` |
| `effect-style` | `<frame>`, `<stack>`, `<row>`, `<col>`, `<grid>`, `<rect>`, `<ellipse>` |
| `fill` | `<frame>`, `<stack>`, `<row>`, `<col>`, `<grid>`, `<text>`, `<rect>`, `<ellipse>`, `<line>` |
| `fill-style` | `<frame>`, `<stack>`, `<row>`, `<col>`, `<grid>`, `<text>`, `<rect>`, `<ellipse>`, `<line>` |
| `filter` | `<frame>`, `<stack>`, `<row>`, `<col>`, `<grid>`, `<group>`, `<text>`, `<img>`, `<rect>`, `<ellipse>`, `<line>` |
| `fit` | `<img>` |
| `flip` | `<frame>`, `<stack>`, `<row>`, `<col>`, `<grid>`, `<group>`, `<text>`, `<img>`, `<rect>`, `<ellipse>`, `<line>` |
| `font-family` | `<text>` |
| `font-optical-sizing` | `<text>` |
| `font-size` | `<text>` |
| `font-smoothing` | `<text>` |
| `font-stretch` | `<text>` |
| `font-style` | `<text>` |
| `font-variation` | `<text>` |
| `font-weight` | `<text>` |
| `gap` | `<stack>`, `<row>`, `<col>` |
| `grid-col-gap` | `<stack>` |
| `grid-columns` | `<stack>` |
| `grid-row-gap` | `<stack>` |
| `grid-rows` | `<stack>` |
| `h` | `<frame>`, `<stack>`, `<row>`, `<col>`, `<grid>`, `<group>`, `<text>`, `<img>`, `<rect>`, `<ellipse>` |
| `href` | `<text>` |
| `image-rendering` | `<img>` |
| `isolation` | `<frame>`, `<stack>`, `<row>`, `<col>`, `<grid>`, `<group>`, `<text>`, `<img>`, `<rect>`, `<ellipse>`, `<line>` |
| `leading-trim` | `<text>` |
| `letter-spacing` | `<text>` |
| `line-height` | `<text>` |
| `list` | `<text>` |
| `list-level` | `<text>` |
| `list-marker` | `<text>` |
| `mask` | `<frame>`, `<stack>`, `<row>`, `<col>`, `<grid>`, `<group>`, `<text>`, `<img>`, `<rect>`, `<ellipse>`, `<line>` |
| `mask-composite` | `<group>` |
| `mask-height` | `<group>` |
| `mask-mode` | `<group>` |
| `mask-src` | `<group>` |
| `mask-width` | `<group>` |
| `mask-x` | `<group>` |
| `mask-y` | `<group>` |
| `max-height` | `<frame>`, `<stack>`, `<row>`, `<col>`, `<grid>`, `<group>`, `<text>`, `<img>`, `<rect>`, `<ellipse>`, `<line>` |
| `max-lines` | `<text>` |
| `max-width` | `<frame>`, `<stack>`, `<row>`, `<col>`, `<grid>`, `<group>`, `<text>`, `<img>`, `<rect>`, `<ellipse>`, `<line>` |
| `min-height` | `<frame>`, `<stack>`, `<row>`, `<col>`, `<grid>`, `<group>`, `<text>`, `<img>`, `<rect>`, `<ellipse>`, `<line>` |
| `min-width` | `<frame>`, `<stack>`, `<row>`, `<col>`, `<grid>`, `<group>`, `<text>`, `<img>`, `<rect>`, `<ellipse>`, `<line>` |
| `name` | `<frame>`, `<stack>`, `<row>`, `<col>`, `<grid>`, `<group>`, `<text>`, `<img>`, `<rect>`, `<ellipse>`, `<line>` |
| `object-position` | `<img>` |
| `opacity` | `<frame>`, `<stack>`, `<row>`, `<col>`, `<grid>`, `<group>`, `<text>`, `<img>`, `<rect>`, `<ellipse>`, `<line>` |
| `outline` | `<frame>`, `<stack>`, `<row>`, `<col>`, `<grid>` |
| `outline-offset` | `<frame>`, `<stack>`, `<row>`, `<col>`, `<grid>` |
| `overflow` | `<text>` |
| `overflow-x` | `<frame>`, `<stack>`, `<row>`, `<col>`, `<grid>` |
| `overflow-y` | `<frame>`, `<stack>`, `<row>`, `<col>`, `<grid>` |
| `p` | `<stack>`, `<row>`, `<col>`, `<grid>` |
| `paragraph-indent` | `<text>` |
| `paragraph-spacing` | `<text>` |
| `pb` | `<stack>`, `<row>`, `<col>`, `<grid>` |
| `pl` | `<stack>`, `<row>`, `<col>`, `<grid>` |
| `pr` | `<stack>`, `<row>`, `<col>`, `<grid>` |
| `pt` | `<stack>`, `<row>`, `<col>`, `<grid>` |
| `radius` | `<frame>`, `<stack>`, `<row>`, `<col>`, `<grid>`, `<img>`, `<rect>` |
| `reverse-z` | `<stack>`, `<row>`, `<col>` |
| `rotation` | `<frame>`, `<stack>`, `<row>`, `<col>`, `<grid>`, `<group>`, `<text>`, `<img>`, `<rect>`, `<ellipse>`, `<line>` |
| `row-gap` | `<grid>` |
| `rows` | `<grid>` |
| `scale-x` | `<frame>`, `<stack>`, `<row>`, `<col>`, `<grid>`, `<group>`, `<text>`, `<img>`, `<rect>`, `<ellipse>`, `<line>` |
| `scale-y` | `<frame>`, `<stack>`, `<row>`, `<col>`, `<grid>`, `<group>`, `<text>`, `<img>`, `<rect>`, `<ellipse>`, `<line>` |
| `shadow` | `<frame>`, `<stack>`, `<row>`, `<col>`, `<grid>`, `<rect>`, `<ellipse>` |
| `skew-x` | `<frame>`, `<stack>`, `<row>`, `<col>`, `<grid>`, `<group>`, `<text>`, `<img>`, `<rect>`, `<ellipse>`, `<line>` |
| `skew-y` | `<frame>`, `<stack>`, `<row>`, `<col>`, `<grid>`, `<group>`, `<text>`, `<img>`, `<rect>`, `<ellipse>`, `<line>` |
| `src` | `<img>` |
| `text-case` | `<text>` |
| `text-decoration-skip-ink` | `<text>` |
| `text-style` | `<text>` |
| `text-underline-offset` | `<text>` |
| `text-wrap` | `<text>` |
| `thickness` | `<line>` |
| `transform-origin` | `<frame>`, `<stack>`, `<row>`, `<col>`, `<grid>`, `<group>`, `<text>`, `<img>`, `<rect>`, `<ellipse>`, `<line>` |
| `truncate` | `<text>` |
| `value` | `<text>` |
| `vertical-align` | `<text>` |
| `visible` | `<frame>`, `<stack>`, `<row>`, `<col>`, `<grid>`, `<group>`, `<text>`, `<img>`, `<rect>`, `<ellipse>`, `<line>` |
| `w` | `<frame>`, `<stack>`, `<row>`, `<col>`, `<grid>`, `<group>`, `<text>`, `<img>`, `<rect>`, `<ellipse>`, `<line>` |
| `white-space` | `<text>` |
| `word-break` | `<text>` |
| `word-spacing` | `<text>` |
| `wrap` | `<stack>`, `<row>`, `<col>` |
| `x` | `<frame>`, `<stack>`, `<row>`, `<col>`, `<grid>`, `<group>`, `<text>`, `<img>`, `<rect>`, `<ellipse>`, `<line>` |
| `y` | `<frame>`, `<stack>`, `<row>`, `<col>`, `<grid>`, `<group>`, `<text>`, `<img>`, `<rect>`, `<ellipse>`, `<line>` |
| `z-index` | `<frame>`, `<stack>`, `<row>`, `<col>`, `<grid>`, `<group>`, `<text>`, `<img>`, `<rect>`, `<ellipse>`, `<line>` |

## Ahead of the vendored spec

Supported here but not yet described by `spec.json`, which predates RFC-0032 and still documents `<grid>` as `columns`/`rows` only.

- `<grid>` — `cols`, `unit`
- `<*>` — `gc`, `gr`, `col-span`, `row-span`, `segment`
