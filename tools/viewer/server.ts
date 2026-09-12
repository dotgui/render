/**
 * A local page for checking one `.gui` by eye: drop a file in, and it is
 * rendered by this repository's native renderer and by kit, side by side.
 *
 *   bun run tools/viewer/server.ts        # then open http://localhost:4173
 *
 * The page is the manual counterpart to `--example compare`. The harness ranks
 * geometry across the whole corpus, and a missing word moves no geometry — so
 * some divergences are only ever found by looking.
 *
 * Both renders come from the same places the harness uses, not from browser
 * builds: the native side is `--example render_png`, and kit's is
 * `tools/kit-rasterize.ts`. The WASM build cannot fetch Google fonts or remote
 * images, which most documents use, so rendering there would compare kit
 * against a handicapped renderer.
 *
 * The native example is rebuilt before every render — a no-op when nothing
 * changed — so the page always shows the working tree, not a stale binary.
 *
 * Local only, like the harness: needs cargo, bun, a Chromium, and a dotgui/kit
 * checkout (DOTGUI_KIT, else ../kit beside this one) with `dist/render.js`
 * built. The server binds to 127.0.0.1.
 */
import { existsSync, mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'fs'
import os from 'os'
import path from 'path'
import { fileURLToPath } from 'url'

const HERE = path.dirname(fileURLToPath(import.meta.url))
const ROOT = path.resolve(HERE, '..', '..')
const KIT = process.env.DOTGUI_KIT ?? path.resolve(ROOT, '..', 'kit')
const PORT = Number(process.env.PORT ?? 4173)
const NATIVE_BIN = path.join(ROOT, 'target', 'release', 'examples', 'render_png')

/** Where documents can be picked from without dropping a file. */
const LIBRARY_DIRS = ['examples', 'crates/renderer/tests/fixtures']

interface RenderResult {
  ok: boolean
  /** The PNG as a data URI, when the render produced one. */
  png?: string
  ms: number
  /** Everything the renderer said on stderr: font warnings, failed loads. */
  notes: string[]
  error?: string
}

/** Runs a command to completion, capturing both streams. */
async function run(cmd: string[], cwd = ROOT) {
  const proc = Bun.spawn(cmd, { cwd, stdout: 'pipe', stderr: 'pipe' })
  const [stdout, stderr, code] = await Promise.all([
    new Response(proc.stdout).text(),
    new Response(proc.stderr).text(),
    proc.exited,
  ])
  return { code, stdout, stderr }
}

const lines = (text: string) =>
  text
    .split('\n')
    .map((line) => line.trim())
    .filter(Boolean)

/**
 * Each renderer runs one render at a time. Two cargo builds racing on one
 * target directory block each other anyway, and kit launches a whole browser
 * per render. The two renderers share nothing, so they do not wait on each
 * other.
 */
function serialQueue() {
  let tail: Promise<unknown> = Promise.resolve()
  return <T>(task: () => Promise<T>): Promise<T> => {
    const next = tail.then(task, task)
    tail = next.catch(() => {})
    return next
  }
}
const nativeQueue = serialQueue()
const kitQueue = serialQueue()

/**
 * Writes the uploaded bytes where a subprocess can read them.
 *
 * Renderers name the file in their messages, and the temporary path means
 * nothing to whoever dropped the document in, so it is swapped for the name
 * they gave it.
 */
async function withTempInput(
  bytes: Uint8Array,
  name: string,
  use: (file: string) => Promise<RenderResult>,
): Promise<RenderResult> {
  const dir = mkdtempSync(path.join(os.tmpdir(), 'dotgui-viewer-'))
  // The renderers tell a package from bare markup partly by extension.
  const ext = name.toLowerCase().endsWith('.guix') ? '.guix' : '.gui'
  const file = path.join(dir, `input${ext}`)
  writeFileSync(file, bytes)
  try {
    const result = await use(file)
    const scrub = (text: string) => text.split(file).join(name)
    return {
      ...result,
      notes: result.notes.map(scrub),
      error: result.error && scrub(result.error),
    }
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
}

async function renderNative(bytes: Uint8Array, name: string): Promise<RenderResult> {
  const build = await run(['cargo', 'build', '--release', '-q', '-p', 'dotgui-renderer', '--example', 'render_png'])
  if (build.code !== 0) {
    return { ok: false, ms: 0, notes: [], error: `cargo build failed:\n${build.stderr.trim()}` }
  }

  return withTempInput(bytes, name, async (input) => {
    const output = path.join(path.dirname(input), 'native.png')
    const started = performance.now()
    const result = await run([NATIVE_BIN, input, output])
    const ms = Math.round(performance.now() - started)
    const notes = lines(result.stderr)
    if (result.code !== 0 || !existsSync(output)) {
      return { ok: false, ms, notes, error: notes.pop() ?? `render_png exited ${result.code}` }
    }
    return { ok: true, ms, notes, png: dataUri(readFileSync(output)) }
  })
}

async function renderKit(bytes: Uint8Array, name: string): Promise<RenderResult> {
  return withTempInput(bytes, name, async (input) => {
    const output = path.join(path.dirname(input), 'kit.png')
    const started = performance.now()
    const result = await run(['bun', 'run', path.join(ROOT, 'tools', 'kit-rasterize.ts'), input, output])
    const ms = Math.round(performance.now() - started)
    const notes = lines(result.stderr)
    if (result.code !== 0 || !existsSync(output)) {
      return { ok: false, ms, notes, error: notes.pop() ?? `kit-rasterize exited ${result.code}` }
    }
    return { ok: true, ms, notes, png: dataUri(readFileSync(output)) }
  })
}

const dataUri = (png: Uint8Array) => `data:image/png;base64,${Buffer.from(png).toString('base64')}`

/** The documents already in the repository, grouped by folder. */
function library() {
  return LIBRARY_DIRS.map((dir) => ({
    dir,
    files: readdirSync(path.join(ROOT, dir))
      .filter((file) => /\.guix?$/.test(file))
      .sort(),
  }))
}

/** Resolves a library path, refusing anything outside the library folders. */
function libraryFile(requested: string): string | null {
  const resolved = path.resolve(ROOT, requested)
  const allowed = LIBRARY_DIRS.some((dir) => resolved.startsWith(path.join(ROOT, dir) + path.sep))
  return allowed && existsSync(resolved) ? resolved : null
}

/** Same mapping kit-rasterize uses when inlining packaged assets. */
function mimeFor(name: string): string {
  const ext = name.split('.').pop()?.toLowerCase() ?? ''
  if (ext === 'svg') return 'image/svg+xml'
  if (ext === 'jpg' || ext === 'jpeg') return 'image/jpeg'
  if (ext === 'gif') return 'image/gif'
  if (ext === 'png') return 'image/png'
  return 'image/webp'
}

if (!existsSync(path.join(KIT, 'src', 'package', 'index.ts'))) {
  console.error(`no kit checkout at ${KIT}`)
  console.error('set DOTGUI_KIT to a dotgui/kit checkout, or clone it beside this one')
  process.exit(3)
}
const { unpack } = await import(path.join(KIT, 'src', 'package', 'index.ts'))
const KIT_BUNDLE = path.join(KIT, 'dist', 'render.js')

async function bodyBytes(request: Request) {
  return new Uint8Array(await request.arrayBuffer())
}

const json = (value: unknown, status = 200) =>
  new Response(JSON.stringify(value), { status, headers: { 'content-type': 'application/json' } })

Bun.serve({
  hostname: '127.0.0.1',
  port: PORT,
  // A cold cargo build plus a kit render can take a while.
  idleTimeout: 255,
  async fetch(request) {
    const url = new URL(request.url)
    const name = url.searchParams.get('name') ?? 'input.gui'

    if (url.pathname === '/') {
      return new Response(Bun.file(path.join(HERE, 'index.html')))
    }
    if (url.pathname === '/kit/render.js') {
      if (!existsSync(KIT_BUNDLE)) {
        return new Response(`build the kit render bundle: (cd ${KIT} && bun run build:render)`, { status: 404 })
      }
      return new Response(Bun.file(KIT_BUNDLE), { headers: { 'content-type': 'text/javascript' } })
    }
    if (url.pathname === '/api/library') {
      return json(library())
    }
    if (url.pathname === '/api/library/file') {
      const file = libraryFile(url.searchParams.get('path') ?? '')
      return file ? new Response(Bun.file(file)) : new Response('not found', { status: 404 })
    }
    if (request.method === 'POST' && url.pathname === '/api/render/native') {
      const bytes = await bodyBytes(request)
      return json(await nativeQueue(() => renderNative(bytes, name)))
    }
    if (request.method === 'POST' && url.pathname === '/api/render/kit') {
      const bytes = await bodyBytes(request)
      return json(await kitQueue(() => renderKit(bytes, name)))
    }
    if (request.method === 'POST' && url.pathname === '/api/unpack') {
      // For the live kit view: the markup, and assets as data URIs the way
      // kit's `render()` takes them.
      try {
        const pkg = unpack(await bodyBytes(request))
        const assets: Record<string, string> = {}
        for (const [asset, data] of Object.entries(pkg.assets as Record<string, Uint8Array>)) {
          assets[asset] = `data:${mimeFor(asset)};base64,${Buffer.from(data).toString('base64')}`
        }
        return json({ xml: pkg.xml, assets })
      } catch (err) {
        return json({ error: String(err) }, 400)
      }
    }
    return new Response('not found', { status: 404 })
  },
})

console.log(`dotgui viewer on http://localhost:${PORT}`)
console.log(`kit: ${KIT}`)
