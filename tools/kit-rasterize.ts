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
import { readFileSync, writeFileSync, existsSync, mkdtempSync, rmSync } from 'fs'
import os from 'os'
import path from 'path'
import { fileURLToPath, pathToFileURL } from 'url'

/** Same mapping kit's rasterizer uses when inlining packaged assets. */
function mimeFor(name: string): string {
  const ext = name.split('.').pop()?.toLowerCase() ?? ''
  if (ext === 'svg') return 'image/svg+xml'
  if (ext === 'jpg' || ext === 'jpeg') return 'image/jpeg'
  if (ext === 'gif') return 'image/gif'
  if (ext === 'png') return 'image/png'
  return 'image/webp'
}

/** Where a system Chromium usually lives, matching kit's own search. */
const CHROMIUM = [
  '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome',
  '/Applications/Chromium.app/Contents/MacOS/Chromium',
  '/usr/bin/google-chrome',
  '/usr/bin/chromium',
  '/usr/bin/chromium-browser',
]

const HERE = path.dirname(fileURLToPath(import.meta.url))
const KIT = process.env.DOTGUI_KIT ?? path.resolve(HERE, '..', '..', 'kit')

if (!existsSync(path.join(KIT, 'src', 'rasterize', 'index.ts'))) {
  console.error(`no kit checkout at ${KIT}`)
  console.error('set DOTGUI_KIT to a dotgui/kit checkout, or clone it beside this one')
  process.exit(3)
}

const { rasterize } = await import(path.join(KIT, 'src', 'rasterize', 'index.ts'))
const { unpack } = await import(path.join(KIT, 'src', 'package', 'index.ts'))

/** The canonical string every font probe measures, and the size it uses. */
const PROBE_TEXT = 'Handgloves 12345 WAVE'
const PROBE_SIZE = 32

/**
 * What kit's text actually measures, per declared family.
 *
 * The question that matters is not whether kit resolved the family *by name*
 * — a fallback can land on the same physical typeface, and on macOS
 * `system-ui` lands on SF Pro, so an `SF Pro Display` document renders in SF
 * Pro either way. Concluding "different name, different face" is what wrongly
 * wrote off five comparable documents (see #73).
 *
 * So this reports the width kit gets for a fixed string in the full stack it
 * would apply to an element. The caller compares that against its own
 * measurement of the same string: close widths mean the two renderers are
 * drawing metrically the same face, whatever it ended up being called.
 *
 * `resolvedByName` is still reported, because knowing kit had to fall back is
 * worth saying — it just is not grounds for discarding the comparison.
 */
const FONT_CACHE = path.join(os.tmpdir(), 'dotgui-kit-font-probe.json')

/** Bump when the probe changes, so a cache written by older logic is dropped. */
const FONT_PROBE_VERSION = 3

interface DeclaredFont {
  family: string
  source: string
  category: string
}

interface FontReading {
  resolvedByName: boolean
  width: number
}

/** kit's own `fontStack`, mirrored: `"Family", <generic fallback>`. */
function fallbackFor(font: DeclaredFont): string {
  if (font.source === 'system')
    return 'system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif'
  if (font.category === 'serif') return 'serif'
  if (font.category === 'monospace' || /mono|code|console/i.test(font.family))
    return 'ui-monospace, SFMono-Regular, Menlo, Consolas, monospace'
  if (font.category === 'handwriting') return 'cursive'
  return 'sans-serif'
}

async function probeFonts(fonts: DeclaredFont[]): Promise<Record<string, FontReading>> {
  let cache: Record<string, FontReading> = {}
  try {
    const stored = JSON.parse(readFileSync(FONT_CACHE, 'utf8'))
    if (stored.version === FONT_PROBE_VERSION) cache = stored.families
  } catch {
    /* first run, or a stale cache — probe everything */
  }

  const unknown = fonts.filter((font) => !(font.family in cache))
  if (unknown.length) {
    const exe = process.env.PUPPETEER_EXECUTABLE_PATH ?? CHROMIUM.find((p) => existsSync(p))
    // No browser means rasterizing already failed; nothing to add.
    if (!exe) return cache

    const puppeteer = (await import(Bun.resolveSync('puppeteer-core', KIT))).default
    const browser = await puppeteer.launch({
      executablePath: exe,
      headless: true,
      args: ['--no-sandbox', '--disable-setuid-sandbox', '--font-render-hinting=none'],
    })
    try {
      const page = await browser.newPage()
      await page.goto('about:blank')
      const found: Record<string, FontReading> = await page.evaluate(
        async (list: (DeclaredFont & { stack: string })[], text: string, size: number) => {
          // Google families are webfonts, not installs, so pull the same
          // stylesheet kit injects before measuring anything.
          const links = list
            .filter((font) => font.source === 'google')
            .map((font) => {
              const link = document.createElement('link')
              link.rel = 'stylesheet'
              link.href =
                `https://fonts.googleapis.com/css2?family=${font.family.replace(/ /g, '+')}&display=swap`
              document.head.appendChild(link)
              return link
            })
          await Promise.all(
            links.map(
              (link) =>
                new Promise((done) => {
                  link.onload = done
                  link.onerror = done
                  setTimeout(done, 5000)
                }),
            ),
          )

          const context = document.createElement('canvas').getContext('2d')!
          const width = (family: string) => {
            context.font = `${size}px ${family}`
            return context.measureText(text).width
          }
          const absent = width(JSON.stringify('NoSuchFamily-zzq9'))

          const out: Record<string, FontReading> = {}
          for (const font of list) {
            // A face in a stylesheet is only fetched once something asks for
            // it; assigning it to a canvas does not count.
            await (document as any).fonts
              .load(`${size}px ${JSON.stringify(font.family)}`)
              .catch(() => {})
            out[font.family] = {
              resolvedByName: width(JSON.stringify(font.family)) !== absent,
              width: +width(font.stack).toFixed(3),
            }
          }
          return out
        },
        unknown.map((font) => ({ ...font, stack: `${JSON.stringify(font.family)}, ${fallbackFor(font)}` })),
        PROBE_TEXT,
        PROBE_SIZE,
      )
      Object.assign(cache, found)
      writeFileSync(
        FONT_CACHE,
        JSON.stringify({ version: FONT_PROBE_VERSION, families: cache }),
      )
    } finally {
      await browser.close().catch(() => {})
    }
  }

  return Object.fromEntries(fonts.filter((f) => cache[f.family]).map((f) => [f.family, cache[f.family]]))
}

const argv = process.argv.slice(2)
const wantBoxes = argv[0] === '--boxes'
const [input, output] = wantBoxes ? [argv[1], null] : argv
if (!input || (!wantBoxes && !output)) {
  console.error('usage: bun run tools/kit-rasterize.ts <input.gui|.guix> <output.png>')
  console.error('       bun run tools/kit-rasterize.ts --boxes <input.gui|.guix>')
  process.exit(2)
}

// `unpack` takes both containers: a ZIP, or a bare .guix with no assets.
const pkg = unpack(new Uint8Array(readFileSync(input)))

if (wantBoxes) {
  console.log(JSON.stringify(await dumpBoxes(pkg)))
  process.exit(0)
}

// scale 1 so the result is in the same coordinate space as the native render;
// comparing at deviceScaleFactor 2 would only add a resample step.
const result = await rasterize(pkg, { format: 'png', scale: 1 })

// Which of the document's declared families this browser cannot actually
// resolve. A document whose fonts kit had to substitute is not comparable:
// the two renderers are measuring different typefaces, so every wrapped line
// and every box that hugs one is legitimately a different size.
//
// `document.fonts.check()` is no use here — it answers true whenever the
// stack has any usable fallback, which is always. Measuring is what settles
// it: request the family with no fallback, and compare against a name that
// certainly does not exist. Identical widths mean the family resolved to the
// same substitute, i.e. it is not installed.
const declared = [...pkg.xml.matchAll(/<font\b[^>]*>/g)]
  .map((tag) => ({
    family: /\bfamily="([^"]+)"/.exec(tag[0])?.[1] ?? '',
    source: /\bsource="([^"]+)"/.exec(tag[0])?.[1] ?? '',
    category: /\bcategory="([^"]+)"/.exec(tag[0])?.[1] ?? '',
  }))
  .filter((font) => font.family)
const unique = [...new Map(declared.map((font) => [font.family, font])).values()]
const readings = unique.length ? await probeFonts(unique) : {}

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

// The comparison harness reads this and compares the widths against its own.
console.log(JSON.stringify({ fonts: readings, probe: { text: PROBE_TEXT, size: PROBE_SIZE } }))
const byFallback = Object.entries(readings).filter(([, r]) => !r.resolvedByName)
if (byFallback.length) {
  console.error(
    `kit reached these by fallback rather than by name: ${byFallback.map(([f]) => f).join(', ')}`,
  )
}
