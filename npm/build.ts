/**
 * Builds the WASM renderer into `pkg/` for this package.
 *
 *   bun run build.ts
 *
 * Needs cargo, the wasm32-unknown-unknown target, and a wasm-bindgen CLI
 * matching the version in Cargo.lock — the same as `tools/canvas`.
 */
import path from 'node:path'
import { fileURLToPath } from 'node:url'

const HERE = path.dirname(fileURLToPath(import.meta.url))
const ROOT = path.resolve(HERE, '..')
const WASM = path.join(ROOT, 'target', 'wasm32-unknown-unknown', 'release', 'dotgui_renderer_wasm.wasm')

/**
 * Settings for the WASM build only; native builds keep their own. Whole-program
 * optimisation inlines across crates, and SIMD128 turns on tiny-skia's vector
 * paths. Every current browser and Node 16.4+ runs WASM SIMD.
 */
const WASM_BUILD = {
  CARGO_PROFILE_RELEASE_LTO: 'fat',
  CARGO_PROFILE_RELEASE_CODEGEN_UNITS: '1',
  CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUSTFLAGS: '-C target-feature=+simd128',
}

async function run(cmd: string[], env: Record<string, string> = {}) {
  const proc = Bun.spawn(cmd, { cwd: ROOT, stdout: 'inherit', stderr: 'inherit', env: { ...process.env, ...env } })
  if ((await proc.exited) !== 0) throw new Error(`failed: ${cmd.join(' ')}`)
}

await run(['cargo', 'build', '--release', '-p', 'dotgui-renderer-wasm', '--target', 'wasm32-unknown-unknown'], WASM_BUILD)
await run(['wasm-bindgen', '--target', 'web', '--out-dir', path.join(HERE, 'pkg'), WASM])
