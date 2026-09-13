/**
 * A local page for the canvas approach: a `.gui` drawn by this repository's
 * renderer running as WASM in the browser, selected by clicking the canvas and
 * edited through the page's own inspector, redrawn on every change.
 *
 *   bun run tools/canvas/server.ts        # then open http://localhost:4180
 *
 * The WASM build is rebuilt and re-bound on start — a no-op when nothing
 * changed — so the page runs the working tree. Needs cargo, the
 * wasm32-unknown-unknown target, and a wasm-bindgen CLI matching Cargo.lock.
 *
 * The browser renders, but it cannot fetch every byte a document needs in a
 * form the renderer reads: Google's CSS API answers a browser with WOFF2, and
 * the renderer reads TrueType. `/proxy` fetches as the native renderer does, so
 * Google answers with TrueType, and hands the bytes to the page, which gives
 * them to the engine under the original URL.
 *
 * The same goes for `source="system"` fonts, which a native render reads from
 * the host's font directories and a page cannot see. `/fonts` lists the font
 * files installed on this machine, from the directories the renderer searches,
 * and `/font` serves one of them; the renderer decides which it needs, so a
 * font installed here renders here, as it does natively. The server binds to
 * 127.0.0.1.
 */
import { existsSync, readdirSync, statSync } from 'fs'
import os from 'os'
import path from 'path'
import { fileURLToPath } from 'url'

const HERE = path.dirname(fileURLToPath(import.meta.url))
const ROOT = path.resolve(HERE, '..', '..')
const PORT = Number(process.env.PORT ?? 4180)
const EXAMPLES = path.join(ROOT, 'examples')
/** Multi-document fixture packages, kept unzipped; served zipped as `<name>.gui`. */
const PACKAGES = path.join(ROOT, 'crates', 'renderer', 'tests', 'packages')
const WASM = path.join(ROOT, 'target', 'wasm32-unknown-unknown', 'release', 'dotgui_renderer_wasm.wasm')

/** The user agent the native renderer fetches with, so Google serves TrueType. */
const NATIVE_USER_AGENT = 'ureq/3'

async function run(cmd: string[], env: Record<string, string> = {}) {
  const proc = Bun.spawn(cmd, { cwd: ROOT, stdout: 'inherit', stderr: 'inherit', env: { ...process.env, ...env } })
  if ((await proc.exited) !== 0) throw new Error(`failed: ${cmd.join(' ')}`)
}

/**
 * Build settings for the browser bundle only; native builds keep their own.
 * Whole-program optimisation lets the compiler inline across crates, and
 * SIMD128 turns on tiny-skia's vector paths for painting. Every current
 * browser runs WASM SIMD (Safari from 16.4).
 */
const WASM_BUILD = {
  CARGO_PROFILE_RELEASE_LTO: 'fat',
  CARGO_PROFILE_RELEASE_CODEGEN_UNITS: '1',
  CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUSTFLAGS: '-C target-feature=+simd128',
}

await run(['cargo', 'build', '--release', '-p', 'dotgui-renderer-wasm', '--target', 'wasm32-unknown-unknown'], WASM_BUILD)
await run(['wasm-bindgen', '--target', 'web', '--out-dir', path.join(HERE, 'pkg'), WASM])

/** The directories `system_font_dirs` searches, for this platform. */
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

/** Every font file directly inside those directories, as the renderer lists them. */
function installedFonts() {
  return fontDirs().flatMap((dir) => {
    if (!existsSync(dir)) return []
    return readdirSync(dir)
      .filter((name) => /\.(ttf|otf|ttc)$/i.test(name))
      .map((name) => path.join(dir, name))
      .filter((file) => statSync(file).isFile())
  })
}

const STATIC: Record<string, string> = {
  '/': 'index.html',
  '/gui-canvas.js': 'gui-canvas.js',
  '/render-worker.js': 'render-worker.js',
}

async function proxy(url: string | null) {
  if (!url || !url.startsWith('https://')) return new Response('https URLs only', { status: 400 })
  const upstream = await fetch(url, { headers: { 'User-Agent': NATIVE_USER_AGENT } })
  if (!upstream.ok) return new Response(`upstream answered ${upstream.status}`, { status: 502 })
  return new Response(await upstream.arrayBuffer(), {
    headers: {
      'Content-Type': upstream.headers.get('Content-Type') ?? 'application/octet-stream',
      // Fonts and images at a URL do not change under it; let the browser keep
      // them across reloads rather than fetch an 8 MB font every time.
      'Cache-Control': 'public, max-age=86400',
    },
  })
}

Bun.serve({
  hostname: '127.0.0.1',
  port: PORT,
  async fetch(request) {
    const { pathname, searchParams } = new URL(request.url)

    if (pathname in STATIC) return new Response(Bun.file(path.join(HERE, STATIC[pathname])))
    if (pathname.startsWith('/pkg/')) {
      const file = path.join(HERE, 'pkg', path.basename(pathname))
      if (existsSync(file)) return new Response(Bun.file(file))
    }
    if (pathname === '/examples') {
      const names = readdirSync(EXAMPLES).filter((name) => /\.guix?$/.test(name))
      const packages = existsSync(PACKAGES)
        ? readdirSync(PACKAGES).filter((name) => statSync(path.join(PACKAGES, name)).isDirectory()).map((name) => `${name}.gui`)
        : []
      return Response.json([...names, ...packages].sort())
    }
    if (pathname.startsWith('/examples/')) {
      const name = path.basename(decodeURIComponent(pathname))
      const file = path.join(EXAMPLES, name)
      if (existsSync(file)) return new Response(Bun.file(file))
      const dir = path.join(PACKAGES, name.replace(/\.gui$/, ''))
      if (name.endsWith('.gui') && existsSync(dir) && statSync(dir).isDirectory()) {
        // Zipped on request, so the page always sees the working tree.
        const zip = Bun.spawn(['zip', '-q', '-r', '-X', '-', '.'], { cwd: dir, stdout: 'pipe' })
        return new Response(zip.stdout, { headers: { 'Content-Type': 'application/zip' } })
      }
    }
    if (pathname === '/proxy') return proxy(searchParams.get('url'))
    if (pathname === '/fonts') return Response.json(installedFonts())
    if (pathname === '/font') {
      // Only files the listing names, so this serves fonts and nothing else.
      const file = searchParams.get('path') ?? ''
      if (installedFonts().includes(file)) {
        return new Response(Bun.file(file), { headers: { 'Cache-Control': 'public, max-age=86400' } })
      }
      return new Response('not an installed font', { status: 404 })
    }

    return new Response('not found', { status: 404 })
  },
})

console.log(`canvas demo on http://localhost:${PORT}`)
