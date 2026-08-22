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
import os from 'os'
import path from 'path'
import { fileURLToPath } from 'url'

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

/**
 * Families this browser has no real font for.
 *
 * Measured, not asked. `document.fonts.check()` answers true whenever the
 * stack has any usable fallback, which is always — it reported "SF Pro
 * Display" as present on a machine that rendered every glyph in system-ui.
 * So each family is requested with no fallback and compared against a name
 * that cannot exist: equal widths mean both landed on the same substitute.
 *
 * Google families are webfonts, not installed ones, so the probe page pulls
 * the same stylesheet kit injects before measuring. Without that they measure
 * as missing even though kit renders them correctly.
 *
 * Results cache across invocations — the harness runs this script once per
 * document, and which fonts a machine has does not change between them.
 */
const FONT_CACHE = path.join(os.tmpdir(), 'dotgui-kit-font-probe.json')

/** Bump when the probe changes, so a cache written by older logic is dropped.
 *  An earlier version measured webfonts without requesting them and cached
 *  every Google family as missing; the wrong answers outlived the bug. */
const FONT_PROBE_VERSION = 2

interface DeclaredFont {
  family: string
  source: string
}

async function probeFonts(fonts: DeclaredFont[]): Promise<string[]> {
  let cache: Record<string, boolean> = {}
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
    if (!exe) return []

    const puppeteer = (await import(Bun.resolveSync('puppeteer-core', KIT))).default
    const browser = await puppeteer.launch({
      executablePath: exe,
      headless: true,
      args: ['--no-sandbox', '--disable-setuid-sandbox', '--font-render-hinting=none'],
    })
    try {
      const page = await browser.newPage()
      await page.goto('about:blank')
      const found: Record<string, boolean> = await page.evaluate(async (list: DeclaredFont[]) => {
        // Mirror kit: a Google family arrives as a stylesheet, not an install.
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
          context.font = `32px ${JSON.stringify(family)}`
          return context.measureText('Handgloves 12345 WAVE').width
        }
        const absent = width('NoSuchFamily-zzq9')

        const out: Record<string, boolean> = {}
        for (const font of list) {
          // A face declared in a stylesheet is only fetched once something
          // asks for it. Assigning it to a canvas does not count, so the
          // request has to be explicit or every webfont measures as missing.
          await (document as any).fonts.load(`32px ${JSON.stringify(font.family)}`).catch(() => {})
          out[font.family] = width(font.family) !== absent
        }
        return out
      }, unknown)
      Object.assign(cache, found)
      writeFileSync(
        FONT_CACHE,
        JSON.stringify({ version: FONT_PROBE_VERSION, families: cache }),
      )
    } finally {
      await browser.close().catch(() => {})
    }
  }

  return fonts.filter((font) => cache[font.family] === false).map((font) => font.family)
}

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
  }))
  .filter((font) => font.family)
const unique = [...new Map(declared.map((font) => [font.family, font])).values()]
const unresolved = unique.length ? await probeFonts(unique) : []

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

// The comparison harness reads this; a human reading stderr sees the warning.
console.log(JSON.stringify({ unresolvedFonts: unresolved }))
if (unresolved.length) {
  console.error(`kit could not resolve: ${unresolved.join(', ')} — it substituted a fallback`)
}
