export type ImageFormat = 'png' | 'webp' | 'jpeg'

export interface ScreenshotOptions {
  /** Output encoding. Default `png`; `webp` is lossless; `jpeg` is drawn on white. */
  format?: ImageFormat
  /** Device pixels per document pixel. Default 2. */
  scale?: number
  /** For a package: the document to draw, by filename. Default the first. */
  page?: string
  /** Active mode per axis, e.g. `{ theme: 'dark' }` (RFC-0037). */
  mode?: Record<string, string>
  /** Fetch fonts and images over the network. Default true. */
  network?: boolean
}

export interface ScreenshotResult {
  image: Uint8Array
  width: number
  height: number
  /** Fonts that did not resolve, and what was used instead. */
  warnings: string[]
}

/** Loads the WASM module once; every other function awaits it. */
export function ready(): Promise<void>

/**
 * Draws a `.gui` — a package's bytes, or markup — to image bytes. Installed
 * system fonts are read from disk, Google fonts and remote images are fetched,
 * exactly as the native renderer would. Throws when the document does not parse.
 */
export function screenshot(input: Uint8Array | string, options?: ScreenshotOptions): Promise<ScreenshotResult>
