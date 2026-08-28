const URL_PATTERN = /\bhttps?:\/\/[^\s<>"'`]+/gi
const TRAILING_PUNCTUATION = /[),.;:!?]+$/g
const LINK_SCAN_ROWS = 12

const readBufferLine = (buffer, row) => {
  const line = buffer.getLine(row)
  if (!line) return null

  let text = ''
  let positions = []
  let lastContentEnd = 0

  for (let x = 0; x < line.length; x += 1) {
    const cell = line.getCell(x)
    if (!cell || cell.getWidth() === 0) continue

    const chars = cell.getChars() || ' '
    text += chars
    for (let index = 0; index < chars.length; index += 1) {
      positions.push({ x: x + 1, y: row + 1 })
    }
    if (/\S/.test(chars)) lastContentEnd = x + cell.getWidth()
  }

  const trimmedLength = text.trimEnd().length
  text = text.slice(0, trimmedLength)
  positions = positions.slice(0, trimmedLength)

  return {
    text,
    positions,
    reachesRightEdge: lastContentEnd >= line.length,
  }
}

const isSafeHttpUrl = (value) => {
  try {
    const parsed = new URL(value)
    return parsed.protocol === 'http:' || parsed.protocol === 'https:'
  } catch (e) {
    return false
  }
}

export const findHardWrappedUrlLinks = (term, bufferLineNumber, activate) => {
  const buffer = term.buffer.active
  const requestedRow = bufferLineNumber - 1
  const firstRow = Math.max(0, requestedRow - LINK_SCAN_ROWS)
  const lastRow = Math.min(buffer.length - 1, requestedRow + LINK_SCAN_ROWS)
  let combinedText = ''
  const positions = []
  let previous = null

  for (let row = firstRow; row <= lastRow; row += 1) {
    const current = readBufferLine(buffer, row)
    if (!current) break

    // Qoder 这类 TUI 会在恰好占满终端宽度时主动输出换行。只有上一行的
    // 最后一个非空白字符确实抵达最右列时才无缝拼接，否则插入换行阻断匹配。
    if (previous && !previous.reachesRightEdge) {
      combinedText += '\n'
      positions.push(null)
    }
    combinedText += current.text
    positions.push(...current.positions)
    previous = current
  }

  const links = []
  for (const match of combinedText.matchAll(URL_PATTERN)) {
    const url = match[0].replace(TRAILING_PUNCTUATION, '')
    if (!url || !isSafeHttpUrl(url)) continue

    const start = positions[match.index]
    const end = positions[match.index + url.length - 1]
    if (!start || !end || start.y === end.y) continue
    if (bufferLineNumber < start.y || bufferLineNumber > end.y) continue

    links.push({
      range: { start, end },
      text: url,
      decorations: { pointerCursor: true, underline: true },
      activate,
    })
  }
  return links
}

export const registerHardWrappedUrlLinks = (term, activate) => term.registerLinkProvider({
  provideLinks(bufferLineNumber, callback) {
    const links = findHardWrappedUrlLinks(term, bufferLineNumber, activate)
    callback(links.length ? links : undefined)
  },
})
