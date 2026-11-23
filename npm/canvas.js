/**
 * `<gui-canvas>`: a `.gui` document drawn onto a canvas by the WASM renderer.
 *
 *   import '@dotgui/render/canvas'
 *
 *   <gui-canvas src="checkout.gui"></gui-canvas>
 *   <gui-canvas>
 *     <script type="application/gui"> <gui> … </gui> </script>
 *   </gui-canvas>
 *
 * The markup is the source of truth, held as an XML `Document` exactly as it
 * was written. The canvas shows pixels; the element answers questions about
 * them from the layout the renderer returns with every frame, and keeps a
 * transparent layer of the document's text and roles over the canvas, so
 * screen readers, find-in-page and search engines read what the pixels show.
 *
 * A package may hold several documents and a `library.guix` (RFC-0042). The
 * element shows one page at a time: a document, or the library's own page
 * when it has one.
 *
 *   el.load(url) / el.loadBytes(bytes) / el.loadMarkup(xml)
 *   el.pages                        → [{ name, library }] for a package, [] for bare markup
 *   el.page = name                  → show another page; edits to the last one are kept
 *   el.modes                        → { axis: { values, default } } the page declares (RFC-0037)
 *   el.mode = { theme: 'dark' }     → draw in other modes; `setMode(axis, value)` for one axis
 *   el.nodeAt(x, y)                 → id of the element under a point, in document pixels
 *   el.element(id) / el.bounds(id)  → its source Element / { x, y, width, height }
 *   el.apply([{ op, node, … }])     → edit the markup, then redraw
 *   el.source                       → the markup, as text
 *   events: render, error, pages, select
 *
 * Attributes, all optional:
 *   selectable    hover and click to select elements, for editors
 *   density       device pixels per document pixel to draw at (default: the screen's, at least 2)
 *   asset-proxy   prefix for remote fonts and images, e.g. "/proxy?url=", when they lack CORS
 *   font-list     URL answering a JSON list of installed font paths, for `source="system"` fonts
 *   font-file     prefix serving one of those paths, e.g. "/font?path="
 *
 * With none set, Google fonts and images are fetched directly and system fonts
 * fall back to the bundled Inter.
 *
 * An edit repaints only the areas it changed; `render.detail.repainted` says how
 * many, and is null when the page was painted whole.
 *
 * An id is the element's position among layer elements, "0.2.1": it survives
 * attribute edits, and is re-derived whenever the source is replaced.
 *
 * Rendering runs in one worker shared by every element on the page, so a slow
 * frame never holds up typing, scrolling or pointer input. Where the browser can
 * transfer a canvas to a worker, the worker draws into it directly. Hit testing,
 * bounds and edits are answered here, from the markup and the last frame's
 * layout, without waiting on the worker. The one asynchronous edge: setting
 * `source` takes effect once the worker has normalised it, so read `source`
 * back after the `render` event, not straight after the assignment.
 */

/**
 * Where `render-worker.js` lives. Beside this module by default; a bundle that
 * moves this file elsewhere — an IIFE for a CDN — sets it before the first
 * element connects.
 */
export const config = { workerUrl: null }

let worker = null

/**
 * The shared worker, started on first use. A worker script must be
 * same-origin, so one served from a CDN is started through a same-origin
 * module that imports it.
 */
function getWorker() {
  if (worker) return worker
  const url = new URL(config.workerUrl ?? new URL('./render-worker.js', import.meta.url), location.href)
  const script = url.origin === location.origin
    ? url
    : URL.createObjectURL(new Blob([`import ${JSON.stringify(url.href)}`], { type: 'text/javascript' }))
  worker = new Worker(script, { type: 'module' })
  worker.onmessage = ({ data: { id, ok, result, error } }) => {
    const request = waiting.get(id)
    waiting.delete(id)
    if (ok) request.resolve(result)
    else request.reject(new Error(error))
  }
  return worker
}

const waiting = new Map()
let nextRequest = 0
let nextKey = 0

/** Sends one request to the worker and waits for its answer. */
function call(type, payload, transfer = []) {
  const id = nextRequest++
  return new Promise((resolve, reject) => {
    waiting.set(id, { resolve, reject })
    getWorker().postMessage({ id, type, ...payload }, transfer)
  })
}

/** An endpoint attribute as an absolute URL, since the worker resolves against its own script; null when unset. */
const absolute = (value) => (value == null ? null : new URL(value, location.href).href)

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

/**
 * ARIA roles for the role catalog's names (core/roles). A role without a close
 * ARIA match is announced by its own name, as a described group.
 */
const ARIA_ROLES = {
  alert: 'alert',
  banner: 'status',
  breadcrumb: 'navigation',
  button: 'button',
  checkbox: 'checkbox',
  combobox: 'combobox',
  'context-menu': 'menu',
  dialog: 'dialog',
  divider: 'separator',
  'dropdown-menu': 'menu',
  'floating-action-button': 'button',
  hyperlink: 'link',
  'loading-indicator': 'progressbar',
  'navigation-menu': 'navigation',
  pagination: 'navigation',
  'progress-indicator': 'progressbar',
  'radio-button': 'radio',
  'search-bar': 'search',
  select: 'combobox',
  sidebar: 'complementary',
  slider: 'slider',
  switch: 'switch',
  'tab-bar': 'tablist',
  table: 'table',
  'text-area': 'textbox',
  'text-field': 'textbox',
  toast: 'status',
  toolbar: 'toolbar',
  tooltip: 'tooltip',
  'top-navigation-bar': 'navigation',
  tree: 'tree',
}

/** The markup that stands for one layout box, or null when it says nothing. */
function describeBox(box) {
  const { tag, attributes } = box
  if (tag === 'text') {
    const value = attributes.value ?? box.text ?? ''
    if (!value.trim()) return null
    const span = document.createElement('span')
    span.textContent = value
    const size = Number.parseFloat(attributes.size)
    if (size > 0) span.style.fontSize = `${size}px`
    return span
  }
  if (tag === 'img') {
    const label = attributes.alt ?? attributes.name
    if (!label) return null
    const img = document.createElement('div')
    img.setAttribute('role', 'img')
    img.setAttribute('aria-label', label)
    return img
  }
  const role = attributes.role
  if (!role) return null
  const node = document.createElement('div')
  node.setAttribute('role', ARIA_ROLES[role] ?? 'group')
  if (!ARIA_ROLES[role]) node.setAttribute('aria-roledescription', role.replace(/-/g, ' '))
  if (attributes.name) node.setAttribute('aria-label', attributes.name)
  return node
}

const STYLE = `
  :host { display: inline-block; position: relative; line-height: 0; }
  canvas { display: block; max-width: 100%; height: auto; }
  .text { position: absolute; left: 0; top: 0; transform-origin: 0 0; pointer-events: none; overflow: hidden; }
  .text * { position: absolute; box-sizing: border-box; margin: 0; padding: 0; color: transparent; overflow: hidden; }
  .text span { white-space: pre-wrap; line-height: 1.2; font-family: system-ui, sans-serif; }
  .box { position: absolute; pointer-events: none; box-sizing: border-box; }
  .hover { outline: 1px solid #2f7bff; }
  .selected { outline: 2px solid #2f7bff; box-shadow: 0 0 0 1px #fff inset; }
`

/** Lets the module load where there is no DOM — a server render — without defining anything. */
const Element = globalThis.HTMLElement ?? class {}

export class GuiCanvas extends Element {
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
  /** The modes to draw in, `{ axis: value }`; axes left out take the document's default. */
  #mode = {}
  #canvas
  #textLayer
  #hoverBox
  #selectedBox

  constructor() {
    super()
    const root = this.attachShadow({ mode: 'open' })
    root.innerHTML = `<style>${STYLE}</style><canvas aria-hidden="true"></canvas><div class="text"></div><div class="box hover" hidden></div><div class="box selected" hidden></div>`
    this.#canvas = root.querySelector('canvas')
    this.#textLayer = root.querySelector('.text')
    new ResizeObserver(() => this.#scaleText()).observe(this.#canvas)
    this.#hoverBox = root.querySelector('.hover')
    this.#selectedBox = root.querySelector('.selected')

    this.#canvas.addEventListener('pointermove', (event) => {
      if (!this.selectable) return
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
      if (!this.selectable) return
      this.select(this.nodeAt(...this.#point(event)))
    })
  }

  static get observedAttributes() {
    return ['density']
  }

  /** A new density redraws the page at it. */
  attributeChangedCallback(name, previous, value) {
    if (name === 'density' && previous !== value && this.#doc) this.#render()
  }

  /** Whether pointing at the canvas hovers and selects its elements. */
  get selectable() {
    return this.hasAttribute('selectable')
  }

  async connectedCallback() {
    this.#watchDensity()
    await this.#open()
    const src = this.getAttribute('src')
    const inline = this.querySelector('script[type="application/gui"]')
    try {
      if (src) await this.load(src)
      else if (inline) await this.#setSource(inline.textContent)
    } catch (error) {
      this.#emit('error', { message: String(error.message ?? error) })
    }
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
    return absolute(this.getAttribute('font-list'))
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
    const bytes = await step('download', async () => {
      const response = await fetch(url)
      if (!response.ok) throw new Error(`${url}: ${response.status}`)
      return response.arrayBuffer()
    })
    await this.#show(bytes, step)
  }

  /** Shows a packaged `.gui` (ZIP bytes) or bare markup already in memory. */
  async loadBytes(bytes) {
    this.#since = performance.now()
    this.#timings = {}
    await this.#open()
    await call('open', { key: this.#key, fontList: this.#fontList })
    const buffer = bytes instanceof ArrayBuffer ? bytes.slice(0) : bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength)
    await this.#show(buffer, (_, work) => work())
  }

  async #show(bytes, step) {
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

  /**
   * The mode axes the page declares (RFC-0037), with the library's for a
   * package: `{ axis: { values, default } }`.
   */
  get modes() {
    const axes = {}
    const read = (doc) => {
      for (const mode of doc?.getElementsByTagName('mode') ?? []) {
        const name = mode.getAttribute('name')
        const values = (mode.getAttribute('values') ?? '').split(/\s+/).filter(Boolean)
        if (!name || values.length === 0) continue
        const declared = mode.getAttribute('default')
        axes[name] = { values, default: values.includes(declared) ? declared : values[0] }
      }
    }
    const library = this.#package?.library
    if (library?.xml && !this.#isLibrary) read(new DOMParser().parseFromString(library.xml, 'application/xml'))
    read(this.#doc)
    return axes
  }

  get mode() {
    return { ...this.#mode }
  }

  /** Draws in these modes, `{ axis: value }`, replacing the last. */
  set mode(modes) {
    this.#mode = { ...modes }
    if (this.#doc) this.#render()
  }

  /** Draws one axis in `value`, or in the document's default when `value` is null. */
  setMode(axis, value) {
    const next = { ...this.#mode }
    if (value == null) delete next[axis]
    else next[axis] = value
    this.mode = next
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
        const proxy = absolute(this.getAttribute('asset-proxy'))
        const requested = performance.now()
        const library = this.#isLibrary
        const frame = await call('render', {
          key: this.#key,
          xml,
          library,
          density,
          proxy,
          fontFile: absolute(this.getAttribute('font-file')),
          modes: this.#mode,
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
        this.#describe(frame.layout)
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
          // How many changed areas an edit repainted, when it did not repaint the page.
          repainted: frame.repainted ?? null,
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

  /**
   * Rebuilds the layer that says in markup what the canvas shows: each text
   * as transparent text over its box, each image by its `alt`, each declared
   * `role=` as the ARIA role nearest it. Nesting follows the layout, so a
   * button's label sits inside the button. Laid out in document pixels and
   * scaled with the canvas.
   */
  #describe(layout) {
    const layer = this.#textLayer
    layer.replaceChildren()
    const { width, height } = layout.rect
    Object.assign(layer.style, { width: `${width}px`, height: `${height}px` })
    const visit = (box, parent, origin) => {
      if (box.attributes.visible === 'false') return
      const node = describeBox(box)
      if (node) {
        const { x, y, width, height } = box.rect
        Object.assign(node.style, {
          left: `${x - origin.x}px`,
          top: `${y - origin.y}px`,
          width: `${width}px`,
          height: `${height}px`,
        })
        parent.append(node)
        for (const child of box.children) visit(child, node, box.rect)
      } else {
        for (const child of box.children) visit(child, parent, origin)
      }
    }
    visit(layout, layer, { x: 0, y: 0 })
    this.#scaleText()
  }

  /** Keeps the text layer over the canvas as CSS scales it. */
  #scaleText() {
    const width = this.#layout?.rect.width
    if (!width) return
    const shown = this.#canvas.getBoundingClientRect().width
    this.#textLayer.style.transform = shown ? `scale(${shown / width})` : ''
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

if (typeof customElements !== 'undefined' && !customElements.get('gui-canvas')) {
  customElements.define('gui-canvas', GuiCanvas)
}
