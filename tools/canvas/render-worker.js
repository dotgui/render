/**
 * The renderer, off the page's main thread.
 *
 * Every `<gui-canvas>` on a page shares this one worker: one copy of the WASM
 * module, and one cache of fetched fonts and assets. Each element gets its own
 * engine, keyed by the element, and — where the browser can transfer a canvas
 * to a worker — its own `OffscreenCanvas`, which this draws into directly, so
 * a frame's pixels never cross back to the page.
 *
 * Requests arrive as `{ id, type, ...payload }` and are answered with
 * `{ id, ok, result }` or `{ id, ok: false, error }`.
 */
import init, { Engine, normalize_markup } from './pkg/dotgui_renderer_wasm.js'

const wasmReady = init()

/** key → { engine, canvas } */
const engines = new Map()

/**
 * Bytes fetched for any engine, by URL or font path, so switching documents
 * does not download an 8 MB system font again.
 */
const fetched = new Map()
function fetchOnce(key, request) {
  if (!fetched.has(key)) {
    fetched.set(key, fetch(request)
      // A failed fetch is held as empty bytes, so it is not asked for again.
      .then((response) => (response.ok ? response.arrayBuffer() : new ArrayBuffer(0)))
      .catch(() => new ArrayBuffer(0))
      .then((buffer) => new Uint8Array(buffer)))
  }
  return fetched.get(key)
}

/** Font lists by endpoint, fetched once. */
const fontLists = new Map()
function fontList(endpoint) {
  if (!fontLists.has(endpoint)) {
    fontLists.set(endpoint, fetch(endpoint)
      .then((response) => (response.ok ? response.json() : []))
      .catch(() => []))
  }
  return fontLists.get(endpoint)
}

function entry(key) {
  const found = engines.get(key)
  if (!found) throw new Error(`no engine ${key}`)
  return found
}

/** Fetches `[key, request]` pairs and hands each to the engine under its key. */
async function supply(engine, wanted) {
  const bytes = await Promise.all(wanted.map(([key, request]) => fetchOnce(key, request)))
  wanted.forEach(([key], i) => engine.set_asset(key, bytes[i]))
}

/**
 * Fetches the fonts the engine cannot — web fonts through the asset proxy,
 * installed font files through the font endpoint — until it asks for nothing
 * more. Each round can reveal more: a stylesheet names its font files, and a
 * family that is not installed falls back to the next UI font.
 */
async function supplyFonts(engine, xml, library, proxy, fontFile) {
  let supplied = 0
  for (let round = 0; round < 8; round++) {
    const wanted = [
      ...engine.missing_font_urls(xml, library).map((url) => [url, proxy + encodeURIComponent(url)]),
      ...engine.missing_font_files(xml, library).map((file) => [file, fontFile + encodeURIComponent(file)]),
    ]
    if (wanted.length === 0) break
    await supply(engine, wanted)
    supplied += wanted.length
  }
  return supplied
}

/**
 * Paints over the canvas only the areas where `xml` differs from the markup it
 * shows, and answers like a render; `null` when the page has to be painted
 * whole instead.
 */
function patch(engine, canvas, last, xml, library) {
  const started = performance.now()
  const areas = engine.changed_areas(last.xml, xml, library)
  if (areas === undefined) return null
  const context = canvas.getContext('2d')
  let layout = null
  let warnings = []
  let drawMs = 0
  for (let i = 0; i < areas.length; i += 4) {
    const frame = engine.render_area(xml, areas[i], areas[i + 1], areas[i + 2], areas[i + 3], last.drawnDensity, library)
    const drawStarted = performance.now()
    context.putImageData(new ImageData(new Uint8ClampedArray(frame.pixels), frame.width, frame.height), frame.x, frame.y)
    drawMs += performance.now() - drawStarted
    layout ??= frame.layout
    warnings = frame.warnings
    frame.free()
  }
  if (layout === null) {
    // Nothing paints differently, but the layout's attributes may have: the
    // page still answers hit tests from the new one.
    const frame = engine.render_area(xml, 0, 0, 1, 1, last.drawnDensity, library)
    layout = frame.layout
    warnings = frame.warnings
    frame.free()
  }
  return {
    width: last.width,
    height: last.height,
    layout: JSON.parse(layout),
    warnings,
    engineMs: performance.now() - started - drawMs,
    drawMs,
    repainted: areas.length / 4,
  }
}

const handlers = {
  normalize({ xml }) {
    return [normalize_markup(xml)]
  },

  /**
   * A fresh engine for `key`. The canvas is transferred once, on the first
   * open; later opens — loading another document — keep it.
   */
  async open({ key, canvas, fontList: endpoint }) {
    const previous = engines.get(key)
    previous?.engine.free()
    const engine = new Engine()
    engine.set_host_font_files(await fontList(endpoint))
    engines.set(key, { engine, canvas: canvas ?? previous?.canvas ?? null })
    return [null]
  },

  /** Keeps the package's assets and library; answers with its pages. */
  loadPackage({ key, bytes }) {
    return [JSON.parse(entry(key).engine.load_package(new Uint8Array(bytes)))]
  },

  /**
   * Renders once the fonts are in, without waiting for images: fonts decide
   * the layout, images only fill boxes the markup has already sized. The
   * result says how many images are still missing, for the page to fetch with
   * `supplyImages` and render again.
   *
   * `library` says the markup is the package's `library.guix`: its page is
   * drawn, and the documents drawn after it resolve against this markup.
   */
  async render({ key, xml, library, density, proxy, fontFile }) {
    const target = entry(key)
    const { engine, canvas } = target
    const fontsStarted = performance.now()
    const suppliedFonts = await supplyFonts(engine, xml, library, proxy, fontFile)
    const fontsMs = performance.now() - fontsStarted
    const missingImages = engine.missing_image_urls(xml, library).length

    // An edit to the markup the canvas already shows repaints only what it
    // changed. Anything else — another page, another density, fonts or
    // images that have just arrived — paints the page whole.
    const last = target.last
    const patchable = canvas && last && last.xml !== xml && last.library === library &&
      last.density === density && !suppliedFonts && !target.assetsArrived
    if (patchable) {
      const patched = patch(engine, canvas, last, xml, library)
      if (patched) {
        target.last = { ...last, xml }
        return [{ ...patched, density: last.drawnDensity, warnings: patched.warnings, fontsMs, missingImages }]
      }
    }

    const started = performance.now()
    const frame = engine.render(xml, density, library)
    const engineMs = performance.now() - started
    const { width, height } = frame
    // Less than asked for when the page is too long for a canvas at that density.
    const drawnDensity = frame.density
    const layout = JSON.parse(frame.layout)
    const warnings = frame.warnings
    const pixels = frame.pixels
    // The frame owns its pixels inside WASM memory until it is freed; a
    // large screen at 2x is tens of megabytes.
    frame.free()

    const result = { width, height, density: drawnDensity, layout, warnings, engineMs, fontsMs, missingImages }
    target.last = { xml, library, density, drawnDensity, width, height }
    target.assetsArrived = false
    if (!canvas) {
      // No OffscreenCanvas: hand the pixels to the page, without copying.
      return [{ ...result, pixels: pixels.buffer }, [pixels.buffer]]
    }
    const drawStarted = performance.now()
    canvas.width = width
    canvas.height = height
    canvas.getContext('2d').putImageData(new ImageData(new Uint8ClampedArray(pixels.buffer), width, height), 0, 0)
    return [{ ...result, drawMs: performance.now() - drawStarted }]
  },

  async supplyImages({ key, xml, library, proxy }) {
    const target = entry(key)
    const wanted = target.engine.missing_image_urls(xml, library).map((url) => [url, proxy + encodeURIComponent(url)])
    await supply(target.engine, wanted)
    if (wanted.length > 0) target.assetsArrived = true
    return [wanted.length]
  },

  close({ key }) {
    engines.get(key)?.engine.free()
    engines.delete(key)
    return [null]
  },
}

self.onmessage = async ({ data: { id, type, ...payload } }) => {
  try {
    await wasmReady
    const [result, transfer = []] = await handlers[type](payload)
    self.postMessage({ id, ok: true, result }, transfer)
  } catch (error) {
    self.postMessage({ id, ok: false, error: String(error?.message ?? error) })
  }
}
