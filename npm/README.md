# @dotgui/render

The dotgui renderer — `.gui` drawn by Rust, compiled to WebAssembly, in Node and in the browser. The same
code as the native renderer in [dotgui/render](https://github.com/dotgui/render);
it reads documents up to spec 0.4.4.

## Node and Bun

```js
import { screenshot } from '@dotgui/render/node'

const { image, width, height } = await screenshot(guiBytes, { format: 'webp', scale: 2 })
```

`input` is a packaged `.gui` (ZIP bytes) or `.guix` markup. Installed system
fonts are read from disk; Google fonts and remote images are fetched. Options:
`format` (`png`, `webp` lossless, `jpeg`), `scale` (default 2), `page` (a
document by filename), `mode` (e.g. `{ theme: 'dark' }`), `network` (default
true).

## Browser

```js
import '@dotgui/render/canvas'
```

```html
<gui-canvas src="checkout.gui"></gui-canvas>

<gui-canvas>
  <script type="application/gui"> <gui> … </gui> </script>
</gui-canvas>
```

`<gui-canvas>` draws in a worker shared by every element on the page, sized in
document pixels and sharp at the screen's density. Google fonts and images are
fetched directly (WOFF2 is read); `source="system"` families, which a page
cannot see, fall back to the bundled Inter. A transparent layer of the
document's text, image `alt`s and `role=`s sits over the canvas, so screen
readers, find-in-page and search engines read what it shows.

Modes (RFC-0037): `el.modes` lists the declared axes, `el.mode = { theme: 'dark' }`
redraws. Editors add `selectable` and use `nodeAt`, `bounds` and `apply`; see
`canvas.d.ts` for the whole surface, and `asset-proxy`, `font-list` and
`font-file` for hosts that serve their own fonts and assets.

Bundled away from its worker — an IIFE on a CDN, say — set
`config.workerUrl` before the first element connects. The worker may live on
another origin; it is started through a same-origin module.

The engine itself is there too, for hosts that draw on their own:

```js
import { init, Engine } from '@dotgui/render'

await init()
const engine = new Engine()
```

## Building

```bash
bun run build.ts
```

Needs cargo, the `wasm32-unknown-unknown` target and a `wasm-bindgen` CLI
matching `Cargo.lock`.
