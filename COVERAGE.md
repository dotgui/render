# Spec coverage

Generated — do not edit by hand. Run `UPDATE_COVERAGE=1 cargo test -p dotgui-renderer --test spec_coverage` and commit the result.

Measured against `spec/spec.json`, vendored from `dotgui/core` at commit `bdcb1eb7b5f5`. Coverage is declared in `crates/renderer/src/coverage.rs`, not inferred from the sources, so a row says the renderer reads that attribute and acts on it.

**142 of 519** element/attribute pairs implemented. Pairs, not unique attributes — an attribute shared by eight elements counts eight times, because supporting it on one is not supporting it on the others.

| Element | Implemented | Total |
|---|---|---|
| `<frame>` | 10 | 45 |
| `<stack>` | 22 | 59 |
| `<row>` | 17 | 54 |
| `<col>` | 17 | 54 |
| `<grid>` | 17 | 54 |
| `<group>` | 6 | 34 |
| `<text>` | 19 | 72 |
| `<img>` | 10 | 38 |
| `<rect>` | 9 | 39 |
| `<ellipse>` | 8 | 37 |
| `<line>` | 6 | 30 |
| `<appearance>` | 1 | 3 |

## Not implemented

**`<frame>`**

`aspect-ratio`, `blend`, `border-align`, `border-color`, `border-image`, `border-style`, `border-width`, `clip-path`, `constraint-h`, `constraint-v`, `corner-smoothing`, `effect-style`, `fill-style`, `filter`, `flip`, `isolation`, `mask`, `max-height`, `max-width`, `min-height`, `min-width`, `name`, `outline`, `outline-offset`, `overflow-x`, `overflow-y`, `rotation`, `scale-x`, `scale-y`, `shadow`, `skew-x`, `skew-y`, `transform-origin`, `visible`, `z-index`

**`<stack>`**

`aspect-ratio`, `blend`, `border-align`, `border-color`, `border-image`, `border-style`, `border-width`, `clip-path`, `constraint-h`, `constraint-v`, `corner-smoothing`, `effect-style`, `fill-style`, `filter`, `flip`, `isolation`, `mask`, `max-height`, `max-width`, `min-height`, `min-width`, `name`, `outline`, `outline-offset`, `overflow-x`, `overflow-y`, `reverse-z`, `rotation`, `scale-x`, `scale-y`, `shadow`, `skew-x`, `skew-y`, `transform-origin`, `visible`, `wrap`, `z-index`

**`<row>`**

`aspect-ratio`, `blend`, `border-align`, `border-color`, `border-image`, `border-style`, `border-width`, `clip-path`, `constraint-h`, `constraint-v`, `corner-smoothing`, `effect-style`, `fill-style`, `filter`, `flip`, `isolation`, `mask`, `max-height`, `max-width`, `min-height`, `min-width`, `name`, `outline`, `outline-offset`, `overflow-x`, `overflow-y`, `reverse-z`, `rotation`, `scale-x`, `scale-y`, `shadow`, `skew-x`, `skew-y`, `transform-origin`, `visible`, `wrap`, `z-index`

**`<col>`**

`aspect-ratio`, `blend`, `border-align`, `border-color`, `border-image`, `border-style`, `border-width`, `clip-path`, `constraint-h`, `constraint-v`, `corner-smoothing`, `effect-style`, `fill-style`, `filter`, `flip`, `isolation`, `mask`, `max-height`, `max-width`, `min-height`, `min-width`, `name`, `outline`, `outline-offset`, `overflow-x`, `overflow-y`, `reverse-z`, `rotation`, `scale-x`, `scale-y`, `shadow`, `skew-x`, `skew-y`, `transform-origin`, `visible`, `wrap`, `z-index`

**`<grid>`**

`aspect-ratio`, `blend`, `border-align`, `border-color`, `border-image`, `border-style`, `border-width`, `clip-path`, `col-gap`, `constraint-h`, `constraint-v`, `corner-smoothing`, `effect-style`, `fill-style`, `filter`, `flip`, `isolation`, `mask`, `max-height`, `max-width`, `min-height`, `min-width`, `name`, `outline`, `outline-offset`, `overflow-x`, `overflow-y`, `rotation`, `row-gap`, `scale-x`, `scale-y`, `shadow`, `skew-x`, `skew-y`, `transform-origin`, `visible`, `z-index`

**`<group>`**

`aspect-ratio`, `blend`, `constraint-h`, `constraint-v`, `filter`, `flip`, `isolation`, `mask`, `mask-composite`, `mask-height`, `mask-mode`, `mask-src`, `mask-width`, `mask-x`, `mask-y`, `max-height`, `max-width`, `min-height`, `min-width`, `name`, `rotation`, `scale-x`, `scale-y`, `skew-x`, `skew-y`, `transform-origin`, `visible`, `z-index`

**`<text>`**

`aspect-ratio`, `baseline-shift`, `blend`, `constraint-h`, `constraint-v`, `decoration`, `decoration-color`, `decoration-style`, `decoration-thickness`, `direction`, `fill-style`, `filter`, `flip`, `font-feature`, `font-optical-sizing`, `font-postscript`, `font-smoothing`, `font-stretch`, `font-style-name`, `font-variation`, `href`, `isolation`, `leading-trim`, `list`, `list-level`, `list-marker`, `mask`, `max-height`, `max-width`, `min-height`, `min-width`, `name`, `paragraph-indent`, `paragraph-spacing`, `rotation`, `scale-x`, `scale-y`, `skew-x`, `skew-y`, `text-case`, `text-decoration-skip-ink`, `text-rendering`, `text-resize`, `text-underline-offset`, `text-wrap`, `transform-origin`, `vertical-align`, `visible`, `white-space`, `word-break`, `word-spacing`, `writing-mode`, `z-index`

**`<img>`**

`aspect-ratio`, `blend`, `border-align`, `border-color`, `border-style`, `border-width`, `constraint-h`, `constraint-v`, `corner-smoothing`, `filter`, `flip`, `image-rendering`, `isolation`, `mask`, `max-height`, `max-width`, `min-height`, `min-width`, `name`, `object-position`, `rotation`, `scale-x`, `scale-y`, `skew-x`, `skew-y`, `transform-origin`, `visible`, `z-index`

**`<rect>`**

`aspect-ratio`, `blend`, `border-align`, `border-color`, `border-style`, `border-width`, `constraint-h`, `constraint-v`, `corner-smoothing`, `effect-style`, `fill-rule`, `fill-style`, `filter`, `flip`, `isolation`, `mask`, `max-height`, `max-width`, `min-height`, `min-width`, `name`, `rotation`, `scale-x`, `scale-y`, `shadow`, `skew-x`, `skew-y`, `transform-origin`, `visible`, `z-index`

**`<ellipse>`**

`aspect-ratio`, `blend`, `border-align`, `border-color`, `border-style`, `border-width`, `constraint-h`, `constraint-v`, `effect-style`, `fill-rule`, `fill-style`, `filter`, `flip`, `isolation`, `mask`, `max-height`, `max-width`, `min-height`, `min-width`, `name`, `rotation`, `scale-x`, `scale-y`, `shadow`, `skew-x`, `skew-y`, `transform-origin`, `visible`, `z-index`

**`<line>`**

`aspect-ratio`, `blend`, `constraint-h`, `constraint-v`, `direction`, `fill-style`, `filter`, `flip`, `isolation`, `mask`, `max-height`, `max-width`, `min-height`, `min-width`, `name`, `rotation`, `scale-x`, `scale-y`, `skew-x`, `skew-y`, `thickness`, `transform-origin`, `visible`, `z-index`

**`<appearance>`**

`<border>`, `<fill>`

## Ahead of the vendored spec

Supported here but not yet described by `spec.json`, which predates RFC-0032 and still documents `<grid>` as `columns`/`rows` only.

- `<grid>` — `cols`, `unit`
- `<*>` — `gc`, `gr`, `col-span`, `row-span`, `segment`
