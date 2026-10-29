# @dotgui/render

The dotgui renderer — `.gui` drawn by Rust, compiled to WebAssembly. The same
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
import { init, Engine } from '@dotgui/render'

await init()
const engine = new Engine()
```

The browser cannot read installed fonts, and Google answers it with WOFF2,
which the renderer does not read — a host supplies font bytes through
`engine.set_asset`, as `tools/canvas` in the repository shows.

## Building

```bash
bun run build.ts
```

Needs cargo, the `wasm32-unknown-unknown` target and a `wasm-bindgen` CLI
matching `Cargo.lock`.
