/** Where the shared worker script lives; set before the first `<gui-canvas>` connects when this module is bundled away from it. */
export declare const config: { workerUrl: string | URL | null }

export interface GuiPage {
  name: string
  library: boolean
}

export interface ModeAxis {
  values: string[]
  default: string
}

export interface Bounds {
  x: number
  y: number
  width: number
  height: number
}

export type CanvasEdit =
  | { op: 'set'; node: string; attr: string; value: string | null }
  | { op: 'text'; node: string; value: string }
  | { op: 'remove'; node: string }

export interface RenderDetail {
  /** Document width and height, in document pixels. */
  width: number
  height: number
  /** Whether every image had arrived. A second `render` follows when they do. */
  complete: boolean
  density: number
  warnings: string[]
  /** Areas an edit repainted, or null when the page was painted whole. */
  repainted: number | null
  ms: number
  wallMs: number | null
}

/**
 * `<gui-canvas>`: a `.gui` document drawn onto a canvas by the WASM renderer,
 * with a transparent layer of its text and roles for assistive technology.
 */
export declare class GuiCanvas extends HTMLElement {
  load(url: string): Promise<void>
  loadBytes(bytes: ArrayBuffer | Uint8Array): Promise<void>
  loadMarkup(xml: string): Promise<void>

  readonly pages: GuiPage[]
  page: string | null

  /** Mode axes the page declares (RFC-0037), with the library's for a package. */
  readonly modes: Record<string, ModeAxis>
  /** The modes drawn in; axes left out take the document's default. */
  mode: Record<string, string>
  setMode(axis: string, value: string | null): void

  source: string
  readonly document: XMLDocument | null
  readonly density: number
  readonly selectable: boolean
  readonly selected: string | null

  nodeAt(x: number, y: number): string | null
  element(id: string): Element | null
  idOf(element: Element): string | null
  bounds(id: string | null): Bounds | null
  select(id: string | null): void
  apply(edits: CanvasEdit[]): void

  addEventListener(type: 'render', listener: (event: CustomEvent<RenderDetail>) => void, options?: boolean | AddEventListenerOptions): void
  addEventListener(type: 'error', listener: (event: CustomEvent<{ message: string }>) => void, options?: boolean | AddEventListenerOptions): void
  addEventListener(type: 'pages', listener: (event: CustomEvent<{ pages: GuiPage[]; page: string | null }>) => void, options?: boolean | AddEventListenerOptions): void
  addEventListener(type: 'select', listener: (event: CustomEvent<{ id: string | null; element: Element | null }>) => void, options?: boolean | AddEventListenerOptions): void
  addEventListener(type: string, listener: EventListenerOrEventListenerObject, options?: boolean | AddEventListenerOptions): void
}

declare global {
  interface HTMLElementTagNameMap {
    'gui-canvas': GuiCanvas
  }
}
