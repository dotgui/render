/**
 * @dotgui/render — the dotgui renderer, compiled to WebAssembly.
 *
 * In a browser, call `init()` once (it fetches the .wasm beside this module),
 * then use `Engine`. In Node or Bun, use `@dotgui/render/node`, which loads the
 * .wasm from disk and supplies fonts and images for you.
 */
export { default as init } from './pkg/dotgui_renderer_wasm.js'
export * from './pkg/dotgui_renderer_wasm.js'
