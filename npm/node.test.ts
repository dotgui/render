import { describe, expect, test } from 'bun:test'
import { readFileSync, readdirSync } from 'node:fs'
import path from 'node:path'
import { screenshot } from './node.js'

const EXAMPLES = path.join(import.meta.dir, '..', 'examples')
const example = readdirSync(EXAMPLES).filter((f) => f.endsWith('.gui')).sort()[0]
const MAGIC: Record<string, number[]> = { png: [0x89, 0x50, 0x4e, 0x47], jpeg: [0xff, 0xd8, 0xff], webp: [0x52, 0x49, 0x46, 0x46] }

describe('screenshot in Node (offline)', () => {
  test('markup draws to each format at the size it lays out to', async () => {
    const xml = '<gui version="0.4.4"><col w="120" h="40" fill="#3B5BFD" /></gui>'
    for (const format of ['png', 'webp', 'jpeg'] as const) {
      const { image, width, height } = await screenshot(xml, { format, scale: 2, network: false })
      expect([...image.slice(0, MAGIC[format].length)]).toEqual(MAGIC[format])
      expect([width, height]).toEqual([120, 40])
    }
  })

  test('a packaged example draws', async () => {
    const { image, width, height } = await screenshot(new Uint8Array(readFileSync(path.join(EXAMPLES, example))), { format: 'webp', network: false })
    expect(image.length).toBeGreaterThan(1000)
    expect(width).toBeGreaterThan(0)
    expect(height).toBeGreaterThan(0)
  })

  test('a document that does not parse throws', async () => {
    await expect(screenshot('<gui version="9.9"><col /></gui>', { network: false })).rejects.toThrow()
  })
})
