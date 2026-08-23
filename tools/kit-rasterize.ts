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

if (!existsSync(path.join(KIT, 'src', 'package', 'index.ts'))) {
  console.error(`no kit checkout at ${KIT}`)
  console.error('set DOTGUI_KIT to a dotgui/kit checkout, or clone it beside this one')
  process.exit(3)
}

const { unpack } = await import(path.join(KIT, 'src', 'package', 'index.ts'))

/**
 * Renders a document through kit and hands the page to `read`.
 *
 * `rasterize()` from kit would do the screenshot, but it waits 400ms of
 * network idle with a 5s cap and takes no option to change that. That is not
 * enough for a document pulling a large remote image: `relay-dispatch` fetches
 * a 1760px map, kit screenshotted the empty box, and the comparison reported a
 * 19.5% pixel difference that was entirely the harness's own impatience. With
 * a longer wait every request completes and nothing fails.
 *
 * So the page is ours, and the rendering is still kit's — this calls kit's
 * exported `render()`, and the scaffold around it is a div. Both the
 * screenshot and the box dump come through here, so they cannot drift apart.
 */
const NETWORK_IDLE_MS = 1500
const NETWORK_TIMEOUT_MS = 30_000

async function inKitPage<T>(
  pkg: { xml: string; assets: Record<string, Uint8Array> },
  read: (page: any) => Promise<T>,
): Promise<T> {
  const exe = process.env.PUPPETEER_EXECUTABLE_PATH ?? CHROMIUM.find((p) => existsSync(p))
  if (!exe) {
    console.error('no Chromium-based browser found')
    console.error('install a Chromium/Chrome browser, or set PUPPETEER_EXECUTABLE_PATH')
    process.exit(3)
  }
  const bundle = path.join(KIT, 'dist', 'render.js')
  if (!existsSync(bundle)) {
    console.error(`build the kit render bundle: (cd ${KIT} && bun run build:render)`)
    process.exit(3)
  }

  const assetMap: Record<string, string> = {}
  for (const [name, data] of Object.entries(pkg.assets)) {
    assetMap[name] = `data:${mimeFor(name)};base64,${Buffer.from(data).toString('base64')}`
  }

  const tmp = mkdtempSync(path.join(os.tmpdir(), 'dotgui-kit-page-'))
  const harness = path.join(tmp, 'harness.html')
  writeFileSync(
    harness,
    `<!doctype html><html><head><meta charset="utf-8">
<style>html,body{margin:0;padding:0;background:transparent}#root{display:inline-block}</style>
</head><body><div id="root"></div><script type="module">
  import { render } from ${JSON.stringify(pathToFileURL(bundle).href)}
  window.__render = (xml, assets) => { render(xml, document.getElementById('root'), assets) }
  window.__ready = true
</script></body></html>`,
  )

  const puppeteer = (await import(Bun.resolveSync('puppeteer-core', KIT))).default
  const browser = await puppeteer.launch({
    executablePath: exe,
    headless: true,
    args: ['--no-sandbox', '--disable-setuid-sandbox', '--font-render-hinting=none',
           '--allow-file-access-from-files'],
  })
  try {
    const page = await browser.newPage()
    // Anything that does not arrive is worth saying out loud: a missing image
    // reads as a rendering difference otherwise.
    const failures: string[] = []
    page.on('requestfailed', (request: any) =>
      failures.push(`${request.failure()?.errorText} ${request.url().slice(0, 100)}`))
    page.on('response', (response: any) => {
      if (response.status() >= 400) failures.push(`HTTP ${response.status()} ${response.url().slice(0, 100)}`)
    })

    await page.setViewport({ width: 1600, height: 2400, deviceScaleFactor: 1 })
    await page.goto(pathToFileURL(harness).href, { waitUntil: 'load' })
    await page.waitForFunction('window.__ready === true', { timeout: 5000 })
    await page.evaluate((xml: string, assets: Record<string, string>) =>
      (window as any).__render(xml, assets), pkg.xml, assetMap)
    await page
      .waitForNetworkIdle({ idleTime: NETWORK_IDLE_MS, timeout: NETWORK_TIMEOUT_MS })
      .catch(() => failures.push(`network still busy after ${NETWORK_TIMEOUT_MS}ms`))

    // An <img> that never decoded would be drawn as empty space.
    const undecoded: string[] = await page.evaluate(() =>
      Array.from(document.querySelectorAll('img'))
        .filter((img) => !(img as HTMLImageElement).complete || (img as HTMLImageElement).naturalWidth === 0)
        .map((img) => (img as HTMLImageElement).src.slice(0, 100)))

    for (const note of [...failures, ...undecoded.map((src) => `never decoded ${src}`)]) {
      console.error(`kit could not load: ${note}`)
    }

    return await read(page)
  } finally {
    await browser.close().catch(() => {})
    rmSync(tmp, { recursive: true, force: true })
  }
}

/** kit's box geometry for a document, as a flat pre-order list. */
async function dumpBoxes(pkg: { xml: string; assets: Record<string, Uint8Array> }) {
  return inKitPage(pkg, (page) =>
    page.evaluate(() => {
      const root = document.querySelector('#root > *') as HTMLElement
      if (!root) return { boxes: [] }
      const base = root.getBoundingClientRect()
      const boxes: unknown[] = []
      const walk = (el: Element, depth: number) => {
        const r = el.getBoundingClientRect()
        // kit wraps a `<gui-img>` around a plain `<img>` of identical bounds;
        // it has no counterpart in this renderer's tree.
        if (el.tagName.toLowerCase() !== 'img') {
          boxes.push({
            tag: el.tagName.toLowerCase().replace(/^gui-/, ''),
            depth,
            x: +(r.left - base.left).toFixed(3),
            y: +(r.top - base.top).toFixed(3),
            w: +r.width.toFixed(3),
            h: +r.height.toFixed(3),
          })
        }
        for (const child of Array.from(el.children)) walk(child, depth + 1)
      }
      walk(root, 0)
      return { boxes }
    }))
}

/** kit's rendering of a document, as PNG bytes. */
async function screenshot(pkg: { xml: string; assets: Record<string, Uint8Array> }): Promise<Uint8Array> {
  return inKitPage(pkg, async (page) => {
    const target = (await page.$('#root > *')) ?? (await page.$('#root'))
    if (!target) {
      console.error('kit rendered nothing')
      process.exit(3)
    }
    // scale 1 so the result is in the same coordinate space as the native
    // render; comparing at deviceScaleFactor 2 would only add a resample step.
    return (await target.screenshot({ type: 'png', omitBackground: true })) as Uint8Array
  })
}

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

// Which declared families kit could actually reach, and what its text
// measures in them. The caller compares that against its own measurement.
const declared = [...pkg.xml.matchAll(/<font\b[^>]*>/g)]
  .map((tag) => ({
    family: /\bfamily="([^"]+)"/.exec(tag[0])?.[1] ?? '',
    source: /\bsource="([^"]+)"/.exec(tag[0])?.[1] ?? '',
    category: /\bcategory="([^"]+)"/.exec(tag[0])?.[1] ?? '',
  }))
  .filter((font) => font.family)
const unique = [...new Map(declared.map((font) => [font.family, font])).values()]
const readings = unique.length ? await probeFonts(unique) : {}

writeFileSync(output!, await screenshot(pkg))

// The comparison harness reads this and compares the widths against its own.
console.log(JSON.stringify({ fonts: readings, probe: { text: PROBE_TEXT, size: PROBE_SIZE } }))
const byFallback = Object.entries(readings).filter(([, r]) => !r.resolvedByName)
if (byFallback.length) {
  console.error(
    `kit reached these by fallback rather than by name: ${byFallback.map(([f]) => f).join(', ')}`,
  )
}
