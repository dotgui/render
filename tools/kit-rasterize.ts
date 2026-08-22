/**
 * Rasterize a `.gui` or `.guix` with the kit HTML renderer, for comparison
 * against this repository's native output.
 *
 *   bun run tools/kit-rasterize.ts <input.gui|input.guix> <output.png>
 *
 * kit is the behavioural reference for this renderer, but it is a separate
 * repository and not a build dependency. This script is the whole coupling:
 * it is invoked as a subprocess by `--example compare`, and nothing in the
 * library or the test suite reaches for it.
 *
 * kit is located via DOTGUI_KIT, else ../kit beside this checkout.
 *
 * Rasterizing needs a Chromium on the machine (kit drives it with
 * puppeteer-core) and a built `dist/render.js` in the kit checkout. Both
 * failures are reported on stderr with the fix, and exit 3 distinguishes
 * "kit cannot run here" from "kit ran and disagreed".
 */
import { readFileSync, writeFileSync, existsSync } from 'fs'
import path from 'path'
import { fileURLToPath } from 'url'

const HERE = path.dirname(fileURLToPath(import.meta.url))
const KIT = process.env.DOTGUI_KIT ?? path.resolve(HERE, '..', '..', 'kit')

if (!existsSync(path.join(KIT, 'src', 'rasterize', 'index.ts'))) {
  console.error(`no kit checkout at ${KIT}`)
  console.error('set DOTGUI_KIT to a dotgui/kit checkout, or clone it beside this one')
  process.exit(3)
}

const { rasterize } = await import(path.join(KIT, 'src', 'rasterize', 'index.ts'))
const { unpack } = await import(path.join(KIT, 'src', 'package', 'index.ts'))

const [input, output] = process.argv.slice(2)
if (!input || !output) {
  console.error('usage: bun run tools/kit-rasterize.ts <input.gui|.guix> <output.png>')
  process.exit(2)
}

// `unpack` takes both containers: a ZIP, or a bare .guix with no assets.
const pkg = unpack(new Uint8Array(readFileSync(input)))

// scale 1 so the result is in the same coordinate space as the native render;
// comparing at deviceScaleFactor 2 would only add a resample step.
const result = await rasterize(pkg, { format: 'png', scale: 1 })

if (!result.image) {
  console.error(`kit could not rasterize ${input}: ${result.reason}`)
  if (result.reason === 'no-browser') {
    console.error('install a Chromium-based browser, or set PUPPETEER_EXECUTABLE_PATH')
  } else if (result.reason === 'no-renderer') {
    console.error(`build the kit render bundle: (cd ${KIT} && bun run build:render)`)
  }
  process.exit(3)
}

writeFileSync(output, result.image)
