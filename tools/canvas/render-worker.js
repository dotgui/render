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
  for (let round = 0; round < 8; round++) {
    const wanted = [
      ...engine.missing_font_urls(xml, library).map((url) => [url, proxy + encodeURIComponent(url)]),
      ...engine.missing_font_files(xml, library).map((file) => [file, fontFile + encodeURIComponent(file)]),
    ]
    if (wanted.length === 0) return
    await supply(engine, wanted)
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
    const { engine, canvas } = entry(key)
    const fontsStarted = performance.now()
    await supplyFonts(engine, xml, library, proxy, fontFile)
    const fontsMs = performance.now() - fontsStarted
    const missingImages = engine.missing_image_urls(xml, library).length

    const started = performance.now()
    const frame = engine.render(xml, density, library)
    const engineMs = performance.now() - started
    const { width, height } = frame
    const layout = JSON.parse(frame.layout)
    const warnings = frame.warnings
    const pixels = frame.pixels
    // The frame owns its pixels inside WASM memory until it is freed; a
    // large screen at 2x is tens of megabytes.
    frame.free()

    const result = { width, height, layout, warnings, engineMs, fontsMs, missingImages }
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
    const { engine } = entry(key)
    const wanted = engine.missing_image_urls(xml, library).map((url) => [url, proxy + encodeURIComponent(url)])
    await supply(engine, wanted)
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
