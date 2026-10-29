/**
 * @dotgui/render/node — the WASM renderer in Node or Bun, with the host's
 * fonts and network.
 *
 *   import { screenshot } from '@dotgui/render/node'
 *   const { image } = await screenshot(bytes, { format: 'webp' })
 *
 * The engine cannot read files or fetch anything itself. This module does what
 * the browser canvas's worker does: it lists the font files installed on this
 * machine, hands over the ones a document asks for, fetches Google fonts as
 * TrueType and remote images, and asks again until nothing is missing.
 */
import { existsSync, readdirSync, readFileSync, statSync } from 'node:fs'
import os from 'node:os'
import path from 'node:path'
import init, { Engine } from './pkg/dotgui_renderer_wasm.js'

let loading
/** Loads the WASM module once. */
export function ready() {
  loading ??= init({ module_or_path: readFileSync(new URL('./pkg/dotgui_renderer_wasm_bg.wasm', import.meta.url)) })
  return loading
}

/** The directories the native renderer searches for installed fonts. */
function fontDirs() {
  const home = os.homedir()
  const platform =
    process.platform === 'darwin'
      ? ['/System/Library/Fonts', '/System/Library/Fonts/Supplemental', '/Library/Fonts']
      : process.platform === 'win32'
        ? ['C:\\Windows\\Fonts']
        : ['/usr/share/fonts', '/usr/local/share/fonts', '/usr/share/fonts/truetype']
  return [...platform, path.join(home, 'Library/Fonts'), path.join(home, '.local/share/fonts'), path.join(home, '.fonts')]
}

let installed
/** Every font file directly inside those directories. */
function installedFonts() {
  installed ??= fontDirs().flatMap((dir) => {
    if (!existsSync(dir)) return []
    return readdirSync(dir)
      .filter((name) => /\.(ttf|otf|ttc)$/i.test(name))
      .map((name) => path.join(dir, name))
      .filter((file) => {
        try { return statSync(file).isFile() } catch { return false }
      })
  })
  return installed
}

/** Google answers this user agent with TrueType, which the renderer reads. */
const NATIVE_USER_AGENT = 'ureq/3'

/** Bytes by URL, shared across calls; a failure is held as empty bytes. */
const fetched = new Map()
function fetchBytes(url) {
  if (!fetched.has(url)) {
    fetched.set(url, fetch(url, { headers: { 'User-Agent': NATIVE_USER_AGENT } })
      .then(async (response) => (response.ok ? new Uint8Array(await response.arrayBuffer()) : new Uint8Array(0)))
      .catch(() => new Uint8Array(0)))
  }
  return fetched.get(url)
}

function readFont(file) {
  try { return new Uint8Array(readFileSync(file)) } catch { return new Uint8Array(0) }
}

export async function screenshot(input, options = {}) {
  await ready()
  const { format = 'png', scale = 2, page, mode = {}, network = true } = options
  const engine = new Engine()
  try {
    engine.set_host_font_files(installedFonts())
    for (const [axis, value] of Object.entries(mode)) engine.set_mode(axis, value)

    let xml
    let library = false
    const bytes = typeof input === 'string' ? null : input
    if (bytes && bytes[0] === 0x50 && bytes[1] === 0x4b) {
      const pages = JSON.parse(engine.load_package(bytes))
      const chosen = page === undefined
        ? pages.documents[0]
        : pages.documents.find((d) => d.name === page) ?? (pages.library?.name === page ? pages.library : undefined)
      if (!chosen) throw new Error(page === undefined ? 'the package has no document' : `no page named ${page}`)
      if (chosen.xml == null) throw new Error(chosen.error ?? `${chosen.name} is not readable`)
      xml = chosen.xml
      library = chosen === pages.library
    } else {
      xml = typeof input === 'string' ? input : new TextDecoder().decode(input)
    }

    // Each round can reveal more: a stylesheet names its font files, and a
    // family that is not installed falls back to the next UI font.
    for (let round = 0; round < 8; round++) {
      const urls = network ? engine.missing_font_urls(xml, library) : []
      const files = engine.missing_font_files(xml, library)
      if (urls.length === 0 && files.length === 0) break
      const got = await Promise.all(urls.map(fetchBytes))
      urls.forEach((url, i) => engine.set_asset(url, got[i]))
      for (const file of files) engine.set_asset(file, readFont(file))
    }
    if (network) {
      const urls = engine.missing_image_urls(xml, library)
      const got = await Promise.all(urls.map(fetchBytes))
      urls.forEach((url, i) => engine.set_asset(url, got[i]))
    }

    const frame = engine.render(xml, 1, library)
    const { width, height } = frame
    const warnings = frame.warnings
    frame.free()
    const image = engine.screenshot_page_image(xml, scale, library, format)
    return { image, width, height, warnings }
  } finally {
    engine.free()
  }
}
