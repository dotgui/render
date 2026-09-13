/**
 * `<gui-canvas>`: a `.gui` document drawn onto a canvas by the WASM renderer.
 *
 * The markup is the source of truth, held as an XML `Document` exactly as it
 * was written. Nothing in the page's DOM mirrors the design; the canvas shows
 * pixels, and the element answers questions about them from the layout the
 * renderer returns with every frame.
 *
 *   <gui-canvas>
 *     <script type="application/gui"> <gui> … </gui> </script>
 *   </gui-canvas>
 *   <gui-canvas src="checkout.gui"></gui-canvas>
 *
 * A package may hold several documents and a `library.guix` (RFC-0042). The
 * element shows one page at a time: a document, or the library's own page
 * when it has one.
 *
 *   el.nodeAt(x, y)                 → id of the element under a point, in document pixels
 *   el.element(id)                  → its source Element
 *   el.bounds(id)                   → { x, y, width, height }
 *   el.apply([{ op, node, … }])     → edit the markup, then redraw
 *   el.source                       → the markup, as text
 *   el.pages                        → [{ name, library }] for a package, [] for bare markup
 *   el.page = name                  → show another page; edits to the last one are kept
 *   el.loadMarkup(xml)              → show bare markup, leaving any loaded package behind
 *   events: select, render, error, pages
 *
 * An id is the element's position among layer elements, "0.2.1": it survives
 * attribute edits, and is re-derived whenever the source is replaced.
 *
 * Rendering runs in a worker (`render-worker.js`), so a slow frame never holds
 * up typing, scrolling or pointer input. Where the browser can transfer a
 * canvas to a worker, the worker draws into it directly. Everything the page
 * asks of the element — hit testing, bounds, edits — is answered here, from the
 * markup and the last frame's layout, without waiting on the worker. The one
 * asynchronous edge: setting `source` takes effect once the worker has
 * normalised it, so read `source` back after the `render` event, not straight
 * after the assignment.
 */

const worker = new Worker(new URL('./render-worker.js', import.meta.url), { type: 'module' })
const waiting = new Map()
let nextRequest = 0
let nextKey = 0

worker.onmessage = ({ data: { id, ok, result, error } }) => {
  const request = waiting.get(id)
  waiting.delete(id)
  if (ok) request.resolve(result)
  else request.reject(new Error(error))
}

/** Sends one request to the worker and waits for its answer. */
function call(type, payload, transfer = []) {
  const id = nextRequest++
  return new Promise((resolve, reject) => {
    waiting.set(id, { resolve, reject })
    worker.postMessage({ id, type, ...payload }, transfer)
  })
}

/** An endpoint attribute as an absolute URL, since the worker resolves against its own script. */
const absolute = (value) => new URL(value, location.href).href

/** Children of `<gui>` that describe the document rather than draw it. */
const METADATA = new Set(['tokens', 'fonts', 'styles', 'components', 'modes'])

/** Visits every element below `<gui>` that draws, with its path id. */
function eachLayer(doc, visit) {
  const walk = (element, id) => {
    visit(element, id)
    ;[...element.children].forEach((child, i) => walk(child, `${id}.${i}`))
  }
  ;[...doc.documentElement.children].forEach((child, i) => {
    if (!METADATA.has(child.localName)) walk(child, String(i))
  })
}

const STYLE = `
  :host { display: inline-block; position: relative; line-height: 0; }
  canvas { display: block; max-width: 100%; height: auto; }
  .box { position: absolute; pointer-events: none; box-sizing: border-box; }
  .hover { outline: 1px solid #2f7bff; }
  .selected { outline: 2px solid #2f7bff; box-shadow: 0 0 0 1px #fff inset; }
`

export class GuiCanvas extends HTMLElement {
  #key = nextKey++
  #opened = null
  /** Whether the worker draws into the canvas; otherwise frames come back as pixels. */
  #offscreen = false
  #doc = null
  #layout = null
  #elements = new Map()
  #selected = null
  #hovered = null
  #rendering = false
  #pending = false
  /** When the change now waiting to be drawn was asked for, for wall-clock timing. */
  #since = null
  /** How the wait for the next frame was spent, by step. */
  #timings = {}
  #supplyingImages = false
  /** The loaded package's pages, `{ documents, library }`, or null for bare markup. */
  #package = null
  /** The name of the page on show, when a package is loaded. */
  #page = null
  /** The page `#doc` was loaded for. */
  #docPage = null
  #canvas
  #hoverBox
  #selectedBox

  constructor() {
    super()
    const root = this.attachShadow({ mode: 'open' })
    root.innerHTML = `<style>${STYLE}</style><canvas></canvas><div class="box hover" hidden></div><div class="box selected" hidden></div>`
    this.#canvas = root.querySelector('canvas')
    this.#hoverBox = root.querySelector('.hover')
    this.#selectedBox = root.querySelector('.selected')

    this.#canvas.addEventListener('pointermove', (event) => {
      const id = this.nodeAt(...this.#point(event))
      if (id === this.#hovered) return
      this.#hovered = id
      this.#place(this.#hoverBox, id !== this.#selected ? id : null)
    })
    this.#canvas.addEventListener('pointerleave', () => {
      this.#hovered = null
      this.#place(this.#hoverBox, null)
    })
    this.#canvas.addEventListener('click', (event) => {
      this.select(this.nodeAt(...this.#point(event)))
    })
  }

  async connectedCallback() {
    this.#watchDensity()
    await this.#open()
    const src = this.getAttribute('src')
    const inline = this.querySelector('script[type="application/gui"]')
    if (src) await this.load(src)
    else if (inline) await this.#setSource(inline.textContent)
  }

  /** Gives this element an engine in the worker, and its canvas, once. */
  #open() {
    this.#opened ??= (() => {
      // A canvas can be transferred only once, and only before it has a context.
      const canvas = this.#canvas.transferControlToOffscreen?.()
      this.#offscreen = Boolean(canvas)
      return call('open', { key: this.#key, canvas, fontList: this.#fontList }, canvas ? [canvas] : [])
    })()
    return this.#opened
  }

  get #fontList() {
    return absolute(this.getAttribute('font-list') ?? '/fonts')
  }

  /** Loads a packaged `.gui` or bare markup from a URL. */
  async load(url) {
    this.#since = performance.now()
    this.#timings = {}
    const step = async (name, work) => {
      const started = performance.now()
      try {
        return await work()
      } finally {
        this.#timings[name] = (this.#timings[name] ?? 0) + performance.now() - started
      }
    }
    await step('open', async () => {
      await this.#open()
      // A new document gets a new engine, so the last one's assets go with it.
      await call('open', { key: this.#key, fontList: this.#fontList })
    })
    const bytes = await step('download', async () => (await fetch(url)).arrayBuffer())
    const head = new Uint8Array(bytes, 0, Math.min(2, bytes.byteLength))
    const isPackage = head[0] === 0x50 && head[1] === 0x4b
    if (!isPackage) {
      this.#package = null
      this.#page = null
      this.#emit('pages', { pages: [], page: null })
      await this.#setSource(new TextDecoder().decode(bytes))
      return
    }
    // The last package's page is not this one's, even when the names match.
    this.#page = null
    this.#docPage = null
    this.#package = await step('unpack', () => call('loadPackage', { key: this.#key, bytes }, [bytes]))
    const first = this.pages[0]
    if (!first) {
      this.#emit('error', { message: 'the package has no page to show' })
      return
    }
    await this.#showPage(first.name)
  }

  /**
   * Shows bare markup as a standalone document. A fresh engine, so a package
   * loaded before — its assets and its library — does not apply to it.
   */
  async loadMarkup(xml) {
    await this.#open()
    await call('open', { key: this.#key, fontList: this.#fontList })
    this.#package = null
    this.#page = null
    this.#docPage = null
    this.#emit('pages', { pages: [], page: null })
    await this.#setSource(xml)
  }

  /** The package's pages in order: its documents, then the library's own page. */
  get pages() {
    if (!this.#package) return []
    const pages = this.#package.documents.map(({ name }) => ({ name, library: false }))
    const library = this.#package.library
    if (library?.hasPage) pages.push({ name: library.name, library: true })
    return pages
  }

  get page() {
    return this.#page
  }

  set page(name) {
    this.#showPage(name)
  }

  #entry(name) {
    const { documents, library } = this.#package ?? {}
    return documents?.find((document) => document.name === name) ??
      (library?.name === name ? library : null)
  }

  /**
   * Whether the markup on show is the library's. Read from the page the
   * markup was loaded for, not the page asked for: switching waits on the
   * worker, and a frame drawn meanwhile is still the old page's.
   */
  get #isLibrary() {
    return Boolean(this.#package?.library && this.#package.library.name === this.#docPage)
  }

  async #showPage(name) {
    const target = this.#entry(name)
    if (!target) throw new Error(`no page ${name}`)
    // Keep edits to the page being left, so coming back to it shows them.
    const current = this.#entry(this.#docPage)
    if (current && this.#doc) current.xml = this.source
    this.#page = name
    this.#selected = null
    this.#emit('pages', { pages: this.pages, page: name })
    if (target.xml == null) {
      this.#emit('error', { message: `${name}: ${target.error ?? 'unreadable'}` })
      return
    }
    await this.#setSource(target.xml)
  }

  get source() {
    return this.#doc ? new XMLSerializer().serializeToString(this.#doc) : ''
  }

  set source(xml) {
    this.#setSource(xml)
  }

  async #setSource(xml) {
    this.#since ??= performance.now()
    // `.gui` allows `<frame clip>`; XML does not, so give presence attributes
    // a value first, as the renderer's own parser does.
    const normalizeStarted = performance.now()
    const normalized = await call('normalize', { xml })
    this.#timings.normalize = performance.now() - normalizeStarted
    const doc = new DOMParser().parseFromString(normalized, 'application/xml')
    const failure = doc.querySelector('parsererror')
    if (failure || doc.documentElement.localName !== 'gui') {
      this.#emit('error', { message: failure?.textContent ?? 'the root element is not <gui>' })
      return
    }
    this.#doc = doc
    this.#docPage = this.#page
    this.#index()
    if (this.#selected && !this.#elements.has(this.#selected)) this.#selected = null
    await this.#render()
  }

  /** The parsed markup. Mutate it through `apply` so the canvas follows. */
  get document() {
    return this.#doc
  }

  element(id) {
    return this.#elements.get(id) ?? null
  }

  idOf(element) {
    for (const [id, candidate] of this.#elements) if (candidate === element) return id
    return null
  }

  get selected() {
    return this.#selected
  }

  select(id) {
    this.#selected = id && this.#elements.has(id) ? id : null
    this.#place(this.#selectedBox, this.#selected)
    this.#place(this.#hoverBox, null)
    this.#emit('select', { id: this.#selected, element: this.element(this.#selected) })
  }

  /** The id of the frontmost element whose box holds a point in document pixels. */
  nodeAt(x, y) {
    const hit = (box) => {
      const { x: left, y: top, width, height } = box.rect
      if (box.attributes.visible === 'false') return null
      for (let i = box.children.length - 1; i >= 0; i--) {
        const found = hit(box.children[i])
        if (found) return found
      }
      const inside = x >= left && y >= top && x < left + width && y < top + height
      return inside ? (box.attributes['data-uid'] ?? null) : null
    }
    return this.#layout ? hit(this.#layout) : null
  }

  bounds(id) {
    const find = (box) => {
      if (box.attributes['data-uid'] === id) return box.rect
      for (const child of box.children) {
        const found = find(child)
        if (found) return found
      }
      return null
    }
    return this.#layout && id ? find(this.#layout) : null
  }

  /**
   * Applies edits to the markup and redraws once.
   *
   *   { op: 'set',    node, attr, value }   value null removes the attribute
   *   { op: 'text',   node, value }         a text node's `value`, or its body
   *   { op: 'remove', node }
   */
  apply(edits) {
    this.#since ??= performance.now()
    for (const edit of edits) {
      const element = this.element(edit.node)
      if (!element) throw new Error(`no element ${edit.node}`)
      if (edit.op === 'set') {
        if (edit.value == null) element.removeAttribute(edit.attr)
        else element.setAttribute(edit.attr, edit.value)
      } else if (edit.op === 'text') {
        if (element.hasAttribute('value') || !element.textContent.trim()) element.setAttribute('value', edit.value)
        else element.textContent = edit.value
      } else if (edit.op === 'remove') {
        element.remove()
        this.#index()
      } else {
        throw new Error(`unknown op ${edit.op}`)
      }
    }
    this.#render()
  }

  /** Numbers every layer element by its path below `<gui>`. */
  #index() {
    this.#elements.clear()
    eachLayer(this.#doc, (element, id) => this.#elements.set(id, element))
  }

  /** The markup with every layer stamped with its id, for the renderer. */
  #stamped() {
    const copy = this.#doc.cloneNode(true)
    eachLayer(copy, (element, id) => element.setAttribute('data-uid', id))
    return new XMLSerializer().serializeToString(copy)
  }

  /**
   * Draws the current markup. Only one frame is in the worker at a time;
   * edits that land meanwhile coalesce into a single frame of the latest
   * markup once it returns.
   */
  async #render() {
    if (this.#rendering) {
      this.#pending = true
      return
    }
    this.#rendering = true
    try {
      do {
        this.#pending = false
        const density = this.density
        const xml = this.#stamped()
        const proxy = absolute(this.getAttribute('asset-proxy') ?? '/proxy?url=')
        const requested = performance.now()
        const library = this.#isLibrary
        const frame = await call('render', {
          key: this.#key,
          xml,
          library,
          density,
          proxy,
          fontFile: absolute(this.getAttribute('font-file') ?? '/font?path='),
        })
        if (this.#pending) continue // Stale already: skip straight to the newer markup.

        this.#layout = frame.layout
        if (!this.#offscreen) {
          this.#canvas.width = frame.width
          this.#canvas.height = frame.height
          const pixels = new ImageData(new Uint8ClampedArray(frame.pixels), frame.width, frame.height)
          this.#canvas.getContext('2d').putImageData(pixels, 0, 0)
        }
        // The frame has `density` device pixels per document pixel; the element
        // is sized in document pixels, so the browser maps them 1:1 on a
        // screen of that density instead of stretching a 1x image.
        const { width, height } = frame.layout.rect
        this.#canvas.style.width = `${width}px`
        this.#canvas.style.aspectRatio = `${width} / ${height}`
        this.#place(this.#selectedBox, this.#selected)
        const wallMs = this.#since == null ? null : performance.now() - this.#since
        const timings = {
          ...this.#timings,
          fonts: frame.fontsMs,
          render: frame.engineMs,
          draw: frame.drawMs ?? 0,
          // Round trip less the worker's own work: messaging, copying the layout, queueing.
          transport: performance.now() - requested - frame.fontsMs - frame.engineMs - (frame.drawMs ?? 0),
        }
        this.#since = null
        this.#timings = {}
        this.#emit('render', {
          ms: frame.engineMs + (frame.drawMs ?? 0),
          wallMs,
          timings,
          complete: frame.missingImages === 0,
          engineMs: frame.engineMs,
          offscreen: this.#offscreen,
          density: frame.density ?? density,
          width,
          height,
          pixels: [frame.width, frame.height],
          warnings: frame.warnings,
        })
        if (frame.missingImages > 0) this.#supplyImages(xml, library, proxy)
      } while (this.#pending)
    } catch (error) {
      this.#emit('error', { message: String(error.message ?? error) })
    } finally {
      this.#rendering = false
    }
  }

  /**
   * Fetches the images the last frame went without, then draws again. Failed
   * fetches are held as empty, so this settles instead of retrying forever.
   */
  async #supplyImages(xml, library, proxy) {
    if (this.#supplyingImages) return
    this.#supplyingImages = true
    try {
      const supplied = await call('supplyImages', { key: this.#key, xml, library, proxy })
      if (supplied > 0) this.#render()
    } catch (error) {
      this.#emit('error', { message: String(error.message ?? error) })
    } finally {
      this.#supplyingImages = false
    }
  }

  /**
   * Device pixels per document pixel to render at: the `density` attribute
   * when set, otherwise the screen's own ratio rounded up, and never below 2 —
   * a 1x frame looks soft as soon as the canvas is zoomed or scaled.
   */
  get density() {
    const declared = Number(this.getAttribute('density'))
    if (declared > 0) return Math.min(declared, 4)
    return Math.min(Math.max(2, Math.ceil(window.devicePixelRatio || 1)), 3)
  }

  /** Redraws when the window moves to a screen of another density. */
  #watchDensity() {
    const query = matchMedia(`(resolution: ${window.devicePixelRatio}dppx)`)
    query.addEventListener('change', () => {
      if (this.#doc) this.#render()
      this.#watchDensity()
    }, { once: true })
  }

  /**
   * Document pixels under a pointer event, allowing for CSS scaling. Measured
   * against the layout, not the canvas: a canvas handed to the worker no
   * longer reports the size it is drawn at.
   */
  #point(event) {
    const rect = this.#canvas.getBoundingClientRect()
    const scale = (this.#layout?.rect.width ?? rect.width) / rect.width
    return [(event.clientX - rect.left) * scale, (event.clientY - rect.top) * scale]
  }

  #place(box, id) {
    const rect = this.bounds(id)
    box.hidden = !rect
    if (!rect) return
    const scale = this.#canvas.getBoundingClientRect().width / this.#layout.rect.width || 1
    Object.assign(box.style, {
      left: `${rect.x * scale}px`,
      top: `${rect.y * scale}px`,
      width: `${rect.width * scale}px`,
      height: `${rect.height * scale}px`,
    })
  }

  #emit(type, detail) {
    this.dispatchEvent(new CustomEvent(type, { detail }))
  }
}

customElements.define('gui-canvas', GuiCanvas)
