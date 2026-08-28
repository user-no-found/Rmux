import test from 'node:test'
import assert from 'node:assert/strict'
import { findHardWrappedUrlLinks } from '../src/terminalLinks.js'

const makeTerminal = (rows, cols) => {
  const lines = rows.map((value) => ({
    length: cols,
    getCell(index) {
      const char = value[index] || ''
      return {
        getChars: () => char,
        getWidth: () => 1,
      }
    },
  }))
  return {
    buffer: {
      active: {
        length: lines.length,
        getLine: index => lines[index],
      },
    },
  }
}

test('joins a URL split by an explicit newline at the terminal edge', () => {
  const term = makeTerminal([
    'https://ex',
    'ample.com/',
    'path?x=1',
    '',
  ], 10)

  const links = findHardWrappedUrlLinks(term, 2, () => {})

  assert.equal(links.length, 1)
  assert.equal(links[0].text, 'https://example.com/path?x=1')
  assert.deepEqual(links[0].range, {
    start: { x: 1, y: 1 },
    end: { x: 8, y: 3 },
  })
})

test('does not join lines when the URL does not reach the terminal edge', () => {
  const term = makeTerminal([
    'https://',
    'example.com',
  ], 12)

  assert.deepEqual(findHardWrappedUrlLinks(term, 1, () => {}), [])
  assert.deepEqual(findHardWrappedUrlLinks(term, 2, () => {}), [])
})

test('joins the 246-column Qoder login URL shape', () => {
  const firstLine = 'https://qoder.cn/device/selectAccounts?challenge=EmfMKteEiSdGLoABbamgnjU7_EkHItA8mNIja3-YyzY&challenge_method=S256&nonce=70ba0d6e-842d-4ca2-9339-889dee38b483&machine_id=cf6c6d36-440b-48ea-8dae-308e467212e0&client_id=e883ade2-e6e3-4d6d-adf7-f92cef'
  const suffix = 'f5fdcb'
  assert.equal(firstLine.length, 246)

  const term = makeTerminal([firstLine, suffix, ''], 246)
  const links = findHardWrappedUrlLinks(term, 2, () => {})

  assert.equal(links.length, 1)
  assert.equal(links[0].text, firstLine + suffix)
  assert.deepEqual(links[0].range.end, { x: 6, y: 2 })
})
