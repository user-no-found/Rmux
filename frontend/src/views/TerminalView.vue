<template>
  <div class="terminal-shell">
    <main class="terminal-panel">
      <header class="terminal-tabs">
        <div class="tabs-scroll">
          <div
            v-for="(s, index) in sessions"
            :key="s.session_id"
            :class="['terminal-tab', { active: activeSession === s.session_id }]"
            role="button"
            tabindex="0"
            @click="switchSession(s.session_id)"
            @keydown.enter.prevent="switchSession(s.session_id)"
          >
            <span :class="['status-dot', dotClass(index)]"></span>
            <input
              v-if="editingSession === s.session_id"
              :ref="el => setRenameInputRef(s.session_id, el)"
              v-model="editingName"
              class="tab-name-input"
              maxlength="32"
              @click.stop
              @dblclick.stop
              @keydown.enter.prevent.stop="commitRename(s)"
              @keydown.esc.prevent.stop="cancelRename"
              @blur="commitRename(s)"
            />
            <span v-else class="tab-title" @dblclick.stop="beginRename(s)">{{ sessionTitle(s, index) }}</span>
            <button class="tab-rename" title="重命名" @click.stop="beginRename(s)">✎</button>
            <button class="tab-close" title="关闭标签" @click.stop="closeSession(s.session_id)">×</button>
          </div>
          <button class="terminal-tab add-tab" title="新建标签" @click="createNewSession">+</button>
        </div>

        <div class="toolbar">
          <div class="clipboard-menu">
            <button class="tool-btn clipboard-trigger" title="全局剪切板" @click="toggleClipboard">
              <span class="tool-icon">▤</span>
              <span>剪切板</span>
              <span v-if="clipboardHistory.length" class="history-count">{{ clipboardHistory.length }}</span>
            </button>
            <div v-if="clipboardOpen" class="clipboard-popover">
              <div class="clipboard-head">
                <span>全局剪切板</span>
                <button class="clipboard-close" title="关闭" @click="clipboardOpen = false">×</button>
              </div>
              <div class="clipboard-actions">
                <button :disabled="!clipboardHistory.length" @click="clearClipboardHistory">清空</button>
              </div>
              <div v-if="!clipboardHistory.length" class="clipboard-empty">暂无粘贴记录</div>
              <div v-else class="clipboard-list">
                <button
                  v-for="item in clipboardHistory"
                  :key="item.id"
                  class="clipboard-item"
                  :title="clipboardItemTitle(item)"
                  @click="pasteClipboardHistoryItem(item)"
                >
                  <span :class="['clipboard-kind', item.kind]">{{ item.kind === 'image' ? '图片' : '文本' }}</span>
                  <span class="clipboard-preview">{{ clipboardItemPreview(item) }}</span>
                  <span class="clipboard-time">{{ formatClipboardTime(item.createdAt) }}</span>
                </button>
              </div>
            </div>
          </div>
          <button class="tool-btn" title="新建标签" @click="createNewSession">
            <span class="tool-icon">⊞</span>
            <span>新建标签</span>
          </button>
          <button class="tool-btn" title="设置" @click="router.push('/config')">
            <span class="tool-icon">⚙</span>
            <span>设置</span>
          </button>
          <button class="icon-action" title="新网页打开" @click="openInBrowser">↗</button>
        </div>
      </header>

      <section class="terminal-viewport" ref="termContainer">
        <div v-if="sessions.length === 0" class="empty-state">
          <div class="empty-title">Rmux</div>
          <div class="empty-line">没有正在运行的终端会话</div>
          <button class="primary-command" @click="createNewSession">开启本地终端</button>
        </div>

        <div
          v-for="s in sessions"
          :key="s.session_id"
          :data-session-id="s.session_id"
          :class="['term-wrapper', { active: activeSession === s.session_id }]"
          :ref="el => setTermRef(s.session_id, el)"
        ></div>
      </section>

      <footer class="statusbar">
        <div class="status-left">
          <span class="status-dot online"></span>
          <span>{{ activeSession ? '已连接' : '未连接' }}</span>
          <span v-if="activePath" class="status-path" :title="activePath">{{ activePath }}</span>
        </div>
        <div class="status-center">
          <span class="repo-label">仓库地址:</span>
          <a :href="repoHref" target="_blank" rel="noopener noreferrer" title="https://github.com/user-no-found/Rmux">
            https://github.com/user-no-found/Rmux
          </a>
        </div>
        <div class="status-right">
          <span>{{ activeMeta }}</span>
          <span>xterm-256color</span>
          <span class="lock-icon">▣</span>
        </div>
      </footer>
    </main>

    <div v-if="toastMsg" class="paste-toast">{{ toastMsg }}</div>
  </div>
</template>

<script setup>
import { computed, ref, reactive, onMounted, onBeforeUnmount, nextTick, watch } from 'vue'
import { useRouter } from 'vue-router'
import { Terminal } from 'xterm'
import { FitAddon } from 'xterm-addon-fit'
import 'xterm/css/xterm.css'
import axios from 'axios'
import { API_BASE, WS_BASE } from '../runtimeBase'

const router = useRouter()

const sessions = ref([])
const termContainer = ref(null)
const activeSession = ref(null)
const termRefs = reactive({})
const termInstances = reactive({})
const fitAddons = reactive({})
const wsConnections = reactive({})
const pendingTermWrites = {}
const writeFrameIds = {}
const toastMsg = ref('')
const systemInfo = ref({ hostname: 'localhost', os: 'Linux', arch: 'x86_64' })
const repoHref = 'https://github.com/user-no-found/Rmux'
const clipboardOpen = ref(false)
const clipboardHistory = ref([])
const editingSession = ref(null)
const editingName = ref('')
const renameInputRefs = reactive({})

const dotClasses = ['online', 'blue', 'purple', 'orange']
const CLIPBOARD_HISTORY_LIMIT = 20
const MAX_TEXT_HISTORY_CHARS = 100000
const MAX_TERMINAL_WRITE_CHARS = 128 * 1024
const SESSION_REFRESH_MS = 15000
const COMBINING_CODEPOINT_RANGES = [
  [0x0300, 0x036f], [0x0483, 0x0489], [0x0591, 0x05bd], [0x05bf, 0x05bf],
  [0x05c1, 0x05c2], [0x05c4, 0x05c5], [0x05c7, 0x05c7], [0x0610, 0x061a],
  [0x064b, 0x065f], [0x0670, 0x0670], [0x06d6, 0x06dc], [0x06df, 0x06e4],
  [0x06e7, 0x06e8], [0x06ea, 0x06ed], [0x0711, 0x0711], [0x0730, 0x074a],
  [0x07a6, 0x07b0], [0x07eb, 0x07f3], [0x0816, 0x0819], [0x081b, 0x0823],
  [0x0825, 0x0827], [0x0829, 0x082d], [0x0859, 0x085b], [0x08d3, 0x08e1],
  [0x08e3, 0x0902], [0x093a, 0x093a], [0x093c, 0x093c], [0x0941, 0x0948],
  [0x094d, 0x094d], [0x0951, 0x0957], [0x0962, 0x0963], [0x0981, 0x0981],
  [0x09bc, 0x09bc], [0x09c1, 0x09c4], [0x09cd, 0x09cd], [0x09e2, 0x09e3],
  [0x0a01, 0x0a02], [0x0a3c, 0x0a3c], [0x0a41, 0x0a42], [0x0a47, 0x0a48],
  [0x0a4b, 0x0a4d], [0x0a51, 0x0a51], [0x0a70, 0x0a71], [0x0a75, 0x0a75],
  [0x0a81, 0x0a82], [0x0abc, 0x0abc], [0x0ac1, 0x0ac5], [0x0ac7, 0x0ac8],
  [0x0acd, 0x0acd], [0x0ae2, 0x0ae3], [0x0afa, 0x0aff], [0x0b01, 0x0b01],
  [0x0b3c, 0x0b3c], [0x0b3f, 0x0b3f], [0x0b41, 0x0b44], [0x0b4d, 0x0b4d],
  [0x0b56, 0x0b56], [0x0b62, 0x0b63], [0x0b82, 0x0b82], [0x0bc0, 0x0bc0],
  [0x0bcd, 0x0bcd], [0x0c00, 0x0c00], [0x0c04, 0x0c04], [0x0c3e, 0x0c40],
  [0x0c46, 0x0c48], [0x0c4a, 0x0c4d], [0x0c55, 0x0c56], [0x0c62, 0x0c63],
  [0x0c81, 0x0c81], [0x0cbc, 0x0cbc], [0x0cbf, 0x0cbf], [0x0cc6, 0x0cc6],
  [0x0ccc, 0x0ccd], [0x0ce2, 0x0ce3], [0x0d00, 0x0d01], [0x0d3b, 0x0d3c],
  [0x0d41, 0x0d44], [0x0d4d, 0x0d4d], [0x0d62, 0x0d63], [0x0dca, 0x0dca],
  [0x0dd2, 0x0dd4], [0x0dd6, 0x0dd6], [0x0e31, 0x0e31], [0x0e34, 0x0e3a],
  [0x0e47, 0x0e4e], [0x0eb1, 0x0eb1], [0x0eb4, 0x0eb9], [0x0ebb, 0x0ebc],
  [0x0ec8, 0x0ecd], [0x0f18, 0x0f19], [0x0f35, 0x0f35], [0x0f37, 0x0f37],
  [0x0f39, 0x0f39], [0x0f71, 0x0f7e], [0x0f80, 0x0f84], [0x0f86, 0x0f87],
  [0x0f8d, 0x0f97], [0x0f99, 0x0fbc], [0x0fc6, 0x0fc6], [0x102d, 0x1030],
  [0x1032, 0x1037], [0x1039, 0x103a], [0x103d, 0x103e], [0x1058, 0x1059],
  [0x105e, 0x1060], [0x1071, 0x1074], [0x1082, 0x1082], [0x1085, 0x1086],
  [0x108d, 0x108d], [0x109d, 0x109d], [0x1160, 0x11ff], [0x135d, 0x135f],
  [0x1712, 0x1714], [0x1732, 0x1734], [0x1752, 0x1753], [0x1772, 0x1773],
  [0x17b4, 0x17b5], [0x17b7, 0x17bd], [0x17c6, 0x17c6], [0x17c9, 0x17d3],
  [0x17dd, 0x17dd], [0x180b, 0x180f], [0x1885, 0x1886], [0x18a9, 0x18a9],
  [0x1920, 0x1922], [0x1927, 0x1928], [0x1932, 0x1932], [0x1939, 0x193b],
  [0x1a17, 0x1a18], [0x1a1b, 0x1a1b], [0x1a56, 0x1a56], [0x1a58, 0x1a5e],
  [0x1a60, 0x1a60], [0x1a62, 0x1a62], [0x1a65, 0x1a6c], [0x1a73, 0x1a7c],
  [0x1a7f, 0x1a7f], [0x1ab0, 0x1ace], [0x1b00, 0x1b03], [0x1b34, 0x1b34],
  [0x1b36, 0x1b3a], [0x1b3c, 0x1b3c], [0x1b42, 0x1b42], [0x1b6b, 0x1b73],
  [0x1b80, 0x1b81], [0x1ba2, 0x1ba5], [0x1ba8, 0x1ba9], [0x1bab, 0x1bad],
  [0x1be6, 0x1be6], [0x1be8, 0x1be9], [0x1bed, 0x1bed], [0x1bef, 0x1bf1],
  [0x1c2c, 0x1c33], [0x1c36, 0x1c37], [0x1cd0, 0x1cd2], [0x1cd4, 0x1ce0],
  [0x1ce2, 0x1ce8], [0x1ced, 0x1ced], [0x1cf4, 0x1cf4], [0x1cf8, 0x1cf9],
  [0x1dc0, 0x1dff], [0x200b, 0x200f], [0x202a, 0x202e], [0x2060, 0x2064],
  [0x2066, 0x206f], [0x20d0, 0x20f0], [0x2cef, 0x2cf1], [0x2d7f, 0x2d7f],
  [0x2de0, 0x2dff], [0x302a, 0x302f], [0x3099, 0x309a], [0xa66f, 0xa672],
  [0xa674, 0xa67d], [0xa69e, 0xa69f], [0xa6f0, 0xa6f1], [0xa802, 0xa802],
  [0xa806, 0xa806], [0xa80b, 0xa80b], [0xa825, 0xa826], [0xa8c4, 0xa8c5],
  [0xa8e0, 0xa8f1], [0xa926, 0xa92d], [0xa947, 0xa951], [0xa980, 0xa982],
  [0xa9b3, 0xa9b3], [0xa9b6, 0xa9b9], [0xa9bc, 0xa9bc], [0xa9e5, 0xa9e5],
  [0xaa29, 0xaa2e], [0xaa31, 0xaa32], [0xaa35, 0xaa36], [0xaa43, 0xaa43],
  [0xaa4c, 0xaa4c], [0xaa7c, 0xaa7c], [0xaab0, 0xaab0], [0xaab2, 0xaab4],
  [0xaab7, 0xaab8], [0xaabe, 0xaabf], [0xaac1, 0xaac1], [0xaaec, 0xaaed],
  [0xaaf6, 0xaaf6], [0xabe5, 0xabe5], [0xabe8, 0xabe8], [0xabed, 0xabed],
  [0xfb1e, 0xfb1e], [0xfe00, 0xfe0f], [0xfe20, 0xfe2f], [0xfeff, 0xfeff],
  [0xfff9, 0xfffb], [0x101fd, 0x101fd], [0x102e0, 0x102e0], [0x10376, 0x1037a],
  [0x10a01, 0x10a03], [0x10a05, 0x10a06], [0x10a0c, 0x10a0f], [0x10a38, 0x10a3a],
  [0x10a3f, 0x10a3f], [0x10ae5, 0x10ae6], [0x11001, 0x11001], [0x11038, 0x11046],
  [0x1107f, 0x11081], [0x110b3, 0x110b6], [0x110b9, 0x110ba], [0x11100, 0x11102],
  [0x11127, 0x1112b], [0x1112d, 0x11134], [0x11173, 0x11173], [0x11180, 0x11181],
  [0x111b6, 0x111be], [0x111c9, 0x111cc], [0x1122f, 0x11231], [0x11234, 0x11234],
  [0x11236, 0x11237], [0x1123e, 0x1123e], [0x112df, 0x112df], [0x112e3, 0x112ea],
  [0x11300, 0x11301], [0x1133b, 0x1133c], [0x11340, 0x11340], [0x11366, 0x1136c],
  [0x11370, 0x11374], [0x11438, 0x1143f], [0x11442, 0x11444], [0x11446, 0x11446],
  [0x1145e, 0x1145e], [0x114b3, 0x114b8], [0x114ba, 0x114ba], [0x114bf, 0x114c0],
  [0x114c2, 0x114c3], [0x115b2, 0x115b5], [0x115bc, 0x115bd], [0x115bf, 0x115c0],
  [0x115dc, 0x115dd], [0x11633, 0x1163a], [0x1163d, 0x1163d], [0x1163f, 0x11640],
  [0x116ab, 0x116ab], [0x116ad, 0x116ad], [0x116b0, 0x116b5], [0x116b7, 0x116b7],
  [0x1171d, 0x1171f], [0x11722, 0x11725], [0x11727, 0x1172b], [0x1182f, 0x11837],
  [0x11839, 0x1183a], [0x1193b, 0x1193c], [0x1193e, 0x1193e], [0x11943, 0x11943],
  [0x119d4, 0x119d7], [0x119da, 0x119db], [0x119e0, 0x119e0], [0x11a01, 0x11a0a],
  [0x11a33, 0x11a38], [0x11a3b, 0x11a3e], [0x11a47, 0x11a47], [0x11a51, 0x11a56],
  [0x11a59, 0x11a5b], [0x11a8a, 0x11a96], [0x11a98, 0x11a99], [0x11c30, 0x11c36],
  [0x11c38, 0x11c3d], [0x11c3f, 0x11c3f], [0x11c92, 0x11ca7], [0x11caa, 0x11cb0],
  [0x11cb2, 0x11cb3], [0x11cb5, 0x11cb6], [0x11d31, 0x11d36], [0x11d3a, 0x11d3a],
  [0x11d3c, 0x11d3d], [0x11d3f, 0x11d45], [0x11d47, 0x11d47], [0x11d90, 0x11d91],
  [0x11d95, 0x11d95], [0x11d97, 0x11d97], [0x11ef3, 0x11ef4], [0x13430, 0x13438],
  [0x16af0, 0x16af4], [0x16b30, 0x16b36], [0x16f4f, 0x16f4f], [0x16f8f, 0x16f92],
  [0x16fe4, 0x16fe4], [0x1bc9d, 0x1bc9e], [0x1d167, 0x1d169], [0x1d17b, 0x1d182],
  [0x1d185, 0x1d18b], [0x1d1aa, 0x1d1ad], [0x1d242, 0x1d244], [0x1da00, 0x1da36],
  [0x1da3b, 0x1da6c], [0x1da75, 0x1da75], [0x1da84, 0x1da84], [0x1da9b, 0x1da9f],
  [0x1daa1, 0x1daaf], [0x1e000, 0x1e006], [0x1e008, 0x1e018], [0x1e01b, 0x1e021],
  [0x1e023, 0x1e024], [0x1e026, 0x1e02a], [0x1e130, 0x1e136], [0x1e2ae, 0x1e2ae],
  [0x1e2ec, 0x1e2ef], [0x1e8d0, 0x1e8d6], [0x1e944, 0x1e94a], [0xe0100, 0xe01ef],
]

const WIDE_CODEPOINT_RANGES = [
  [0x1100, 0x115f], [0x2329, 0x232a], [0x2e80, 0xa4cf], [0xac00, 0xd7a3],
  [0xf900, 0xfaff], [0xfe10, 0xfe19], [0xfe30, 0xfe6f], [0xff00, 0xff60],
  [0xffe0, 0xffe6], [0x20000, 0x2fffd], [0x30000, 0x3fffd],
]

const EMOJI_WIDE_CODEPOINT_RANGES = [
  [0x2600, 0x27bf], [0x1f000, 0x1f02f], [0x1f0a0, 0x1f0ff], [0x1f100, 0x1f1ff],
  [0x1f300, 0x1f5ff], [0x1f600, 0x1f64f], [0x1f680, 0x1f6ff], [0x1f700, 0x1f77f],
  [0x1f780, 0x1f7ff], [0x1f800, 0x1f8ff], [0x1f900, 0x1f9ff], [0x1fa70, 0x1faff],
]

const codepointInRanges = (codepoint, ranges) => ranges.some(([start, end]) => codepoint >= start && codepoint <= end)

const emojiAwareUnicodeProvider = {
  version: 'rmux-emoji',
  wcwidth(codepoint) {
    if (codepoint === 0) return 0
    if (codepoint < 32 || (codepoint >= 0x7f && codepoint < 0xa0)) return 0
    if (codepoint === 0x200d || (codepoint >= 0x1f3fb && codepoint <= 0x1f3ff)) return 0
    if (codepointInRanges(codepoint, COMBINING_CODEPOINT_RANGES)) return 0
    if (codepointInRanges(codepoint, EMOJI_WIDE_CODEPOINT_RANGES)) return 2
    if (codepointInRanges(codepoint, WIDE_CODEPOINT_RANGES) && codepoint !== 0x303f) return 2
    return 1
  },
}

const configureTerminalUnicode = (term) => {
  try {
    term.unicode.register(emojiAwareUnicodeProvider)
    term.unicode.activeVersion = emojiAwareUnicodeProvider.version
  } catch (e) {}
}

const authHeaders = () => {
  const token = sessionStorage.getItem('rmux_token')
  return token ? { headers: { Authorization: 'Bearer ' + token } } : { headers: {} }
}

const activeInfo = computed(() => sessions.value.find(s => s.session_id === activeSession.value))
const activePath = computed(() => activeInfo.value?.cwd || '')
const activeMeta = computed(() => {
  const size = activeInfo.value?.size
  const host = systemInfo.value.hostname || 'localhost'
  return `${host}${size ? ` · ${size.cols}x${size.rows}` : ''}`
})

const dotClass = (index) => dotClasses[index % dotClasses.length]

const sessionTitle = (session, index) => {
  if (session?.name) return session.name
  return index === 0 ? '本地终端' : `本地终端 ${index + 1}`
}

const showToast = (msg, duration = 2000) => {
  toastMsg.value = msg
  setTimeout(() => {
    if (toastMsg.value === msg) toastMsg.value = ''
  }, duration)
}

const setTermRef = (id, el) => {
  if (el) termRefs[id] = el
}

const setRenameInputRef = (id, el) => {
  if (el) renameInputRefs[id] = el
}

const sendTerminalMessage = (type, data, sessionId = activeSession.value) => {
  const ws = wsConnections[sessionId]
  if (data && ws?.readyState === WebSocket.OPEN) {
    ws.send(JSON.stringify({ type, data }))
    return true
  }
  return false
}

const sendTerminalPaste = (data, sessionId = activeSession.value) => sendTerminalMessage('paste', data, sessionId)

const shellQuotePath = (path) => `'${String(path).replace(/'/g, `'\\''`)}'`

const makeClipboardId = () => {
  if (window.crypto?.randomUUID) return window.crypto.randomUUID()
  return `${Date.now()}-${Math.random().toString(16).slice(2)}`
}

const normalizeClipboardText = (text) => String(text || '').replace(/\r\n/g, '\n')

const loadClipboardHistory = async () => {
  try {
    const res = await axios.get(`${API_BASE}/api/clipboard`, authHeaders())
    if (res.data?.success && Array.isArray(res.data.data)) {
      clipboardHistory.value = res.data.data.slice(0, CLIPBOARD_HISTORY_LIMIT)
    }
  } catch (e) {
    clipboardHistory.value = []
  }
}

const sortSessionsByCreatedAt = (items) => [...items].sort((a, b) => {
  const ta = Date.parse(a.created_at || '') || 0
  const tb = Date.parse(b.created_at || '') || 0
  return ta - tb
})

const mergeSessionList = (incoming) => {
  const serverById = new Map(incoming.map(session => [session.session_id, session]))
  if (sessions.value.length === 0) return sortSessionsByCreatedAt(incoming)

  const merged = []
  for (const local of sessions.value) {
    const fresh = serverById.get(local.session_id)
    if (fresh) {
      merged.push(fresh)
      serverById.delete(local.session_id)
    }
  }

  const additions = sortSessionsByCreatedAt(Array.from(serverById.values()))
  return [...merged, ...additions]
}

const appendSession = (session) => {
  if (!sessions.value.some(s => s.session_id === session.session_id)) {
    sessions.value.push(session)
  }
}

const toggleClipboard = async () => {
  const next = !clipboardOpen.value
  clipboardOpen.value = next
  if (next) await loadClipboardHistory()
}

const recordClipboardItem = async (item) => {
  if (item.kind === 'text' && normalizeClipboardText(item.text).length > MAX_TEXT_HISTORY_CHARS) {
    return
  }

  const fallback = {
    ...item,
    id: item.id || makeClipboardId(),
    createdAt: item.createdAt || Date.now(),
  }
  const sameFallbackItem = (existing) => {
    if (fallback.kind === 'text') return existing.kind === 'text' && existing.text === fallback.text
    return existing.kind === 'image' && existing.path === fallback.path
  }
  clipboardHistory.value = [
    fallback,
    ...clipboardHistory.value.filter(existing => !sameFallbackItem(existing)),
  ].slice(0, CLIPBOARD_HISTORY_LIMIT)

  try {
    const payload = item.kind === 'image'
      ? { kind: 'image', path: item.path, contentType: item.contentType, size: item.size }
      : { kind: 'text', text: item.text }
    const res = await axios.post(`${API_BASE}/api/clipboard`, payload, authHeaders())
    if (res.data?.success && res.data.data) {
      const fresh = res.data.data
      const sameItem = (existing) => {
        if (fresh.kind === 'text') return existing.kind === 'text' && existing.text === fresh.text
        return existing.kind === 'image' && existing.path === fresh.path
      }
      clipboardHistory.value = [
        fresh,
        ...clipboardHistory.value.filter(existing => !sameItem(existing)),
      ].slice(0, CLIPBOARD_HISTORY_LIMIT)
    }
  } catch (e) {
    // 写入失败不影响粘贴主流程，本地历史已经先行更新。
  }
}

const clearClipboardHistory = async () => {
  try {
    await axios.delete(`${API_BASE}/api/clipboard`, authHeaders())
  } catch (e) {}
  clipboardHistory.value = []
}

const clipboardItemPreview = (item) => {
  if (item.kind === 'image') return item.path?.split('/').pop() || item.path || '图片文件'
  const text = normalizeClipboardText(item.text).replace(/\s+/g, ' ').trim()
  return text || '空文本'
}

const clipboardItemTitle = (item) => {
  if (item.kind === 'image') return item.path || '图片文件'
  return normalizeClipboardText(item.text)
}

const formatClipboardTime = (value) => {
  if (!value) return ''
  const date = new Date(value)
  return date.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })
}

const pasteText = (text, { record = true, sessionId = activeSession.value } = {}) => {
  const normalized = normalizeClipboardText(text)
  if (!normalized) {
    showToast('剪切板没有文本')
    return false
  }
  if (!sendTerminalPaste(normalized, sessionId)) {
    showToast('没有可用终端')
    return false
  }
  const shouldRecord = record && normalized.length <= MAX_TEXT_HISTORY_CHARS
  if (shouldRecord) recordClipboardItem({ kind: 'text', text: normalized })
  showToast(shouldRecord ? '已粘贴文本' : '已粘贴文本，内容较长未加入历史')
  return true
}

const uploadClipboardImage = async (blob) => {
  const formData = new FormData()
  const ext = (blob.type || 'image/png').split('/')[1]?.replace('jpeg', 'jpg') || 'png'
  formData.append('image', blob, `paste.${ext}`)
  const res = await axios.post(`${API_BASE}/api/terminal/clipboard`, formData, authHeaders())
  return res.data?.data?.files?.[0]?.path || ''
}

const pasteImagePath = (path, { recordItem = null, sessionId = activeSession.value } = {}) => {
  if (!path) {
    showToast('图片路径无效')
    return false
  }
  if (!sendTerminalPaste(`${shellQuotePath(path)} `, sessionId)) {
    showToast('没有可用终端')
    return false
  }
  if (recordItem) recordClipboardItem(recordItem)
  showToast('图片路径已输入')
  return true
}

const pasteImageBlob = async (blob, { sessionId = activeSession.value } = {}) => {
  if (!sessionId || !blob) return
  showToast('图片已保存，正在输入路径...')
  try {
    const path = await uploadClipboardImage(blob)
    if (path) {
      pasteImagePath(path, {
        sessionId,
        recordItem: {
          kind: 'image',
          path,
          contentType: blob.type || 'image/png',
          size: blob.size || 0,
        },
      })
    } else {
      showToast('图片粘贴失败')
    }
  } catch (e) {
    showToast('图片粘贴失败: ' + (e.response?.data?.message || e.message || '未知错误'))
  }
}

const imageBlobFromPasteItems = (items) => {
  for (const item of Array.from(items || [])) {
    if (item.type?.startsWith('image/')) return item.getAsFile?.() || null
  }
  return null
}

const isEditableTarget = (target) => {
  const el = target instanceof Element ? target : null
  if (!el) return false
  if (el.closest('.xterm')) return false
  return Boolean(el.closest('input, textarea, select, [contenteditable="true"]'))
}

const sessionIdFromEvent = (e) => {
  const el = e?.target instanceof Element ? e.target : null
  return el?.closest('.term-wrapper')?.dataset?.sessionId || activeSession.value
}

const handledPasteEvents = new WeakSet()
let pendingShortcutPasteMode = null
let pendingShortcutPasteSessionId = null
let pendingShortcutPasteTimer = null
let suppressCtrlVInputUntil = 0

const clearPendingShortcutPaste = () => {
  pendingShortcutPasteMode = null
  pendingShortcutPasteSessionId = null
  if (pendingShortcutPasteTimer) {
    clearTimeout(pendingShortcutPasteTimer)
    pendingShortcutPasteTimer = null
  }
}

const isPasteShortcut = (e) => {
  const key = String(e.key || '').toLowerCase()
  return key === 'v' && (e.ctrlKey || e.metaKey) && !e.altKey && !e.shiftKey
}

const pasteClipboardText = async ({ quiet = false, sessionId = activeSession.value } = {}) => {
  try {
    const text = await navigator.clipboard?.readText?.()
    if (!text && quiet) return
    pasteText(text || '', { sessionId })
  } catch (e) {
    if (!quiet) {
      showToast('文本粘贴失败: ' + (e.message || '请检查浏览器剪切板权限'))
    }
  }
}

const armPasteFallback = (sessionId = activeSession.value) => {
  clearPendingShortcutPaste()
  pendingShortcutPasteMode = 'paste'
  pendingShortcutPasteSessionId = sessionId
  suppressCtrlVInputUntil = Date.now() + 1500
  pendingShortcutPasteTimer = setTimeout(async () => {
    if (pendingShortcutPasteMode !== 'paste') return
    const targetSessionId = pendingShortcutPasteSessionId
    clearPendingShortcutPaste()
    await pasteClipboardText({ quiet: true, sessionId: targetSessionId })
  }, 350)
}

const handlePasteShortcut = (e, sessionId = sessionIdFromEvent(e)) => {
  if (e.type !== 'keydown') return false
  if (isEditableTarget(e.target)) return false

  if (isPasteShortcut(e)) {
    armPasteFallback(sessionId)
    return true
  }

  return false
}

const handleGlobalPasteKeydown = (e) => {
  if (e.defaultPrevented || isEditableTarget(e.target)) return
  const sessionId = sessionIdFromEvent(e)
  if (!sessionId) return

  if (isPasteShortcut(e)) {
    armPasteFallback(sessionId)
  }
}

const handlePaste = (e) => {
  if (handledPasteEvents.has(e)) return
  handledPasteEvents.add(e)

  if (isEditableTarget(e.target)) return

  const mode = pendingShortcutPasteMode
  const sessionId = pendingShortcutPasteSessionId || sessionIdFromEvent(e)
  if (mode) clearPendingShortcutPaste()

  if (mode === 'paste') {
    e.preventDefault()
    e.stopPropagation()
    const imageBlob = imageBlobFromPasteItems(e.clipboardData?.items)
    if (imageBlob) pasteImageBlob(imageBlob, { sessionId })
    else pasteText(e.clipboardData?.getData('text/plain') || '', { sessionId })
    return
  }

  const imageBlob = imageBlobFromPasteItems(e.clipboardData?.items)
  if (imageBlob) {
    e.preventDefault()
    e.stopPropagation()
    pasteImageBlob(imageBlob, { sessionId })
    return
  }

  const text = e.clipboardData?.getData('text/plain')
  if (!text) return

  e.preventDefault()
  e.stopPropagation()
  pasteText(text, { sessionId })
}

const loadSystemInfo = async () => {
  try {
    const res = await axios.get(`${API_BASE}/api/system/info`, authHeaders())
    if (res.data.success) systemInfo.value = res.data.data
  } catch (e) {}
}

const scheduleTerminalFlush = (id) => {
  if (!termInstances[id] || writeFrameIds[id]) return

  writeFrameIds[id] = requestAnimationFrame(() => {
    delete writeFrameIds[id]
    const term = termInstances[id]
    const pending = pendingTermWrites[id] || ''
    if (!term || !pending) {
      pendingTermWrites[id] = ''
      return
    }

    const chunk = pending.slice(0, MAX_TERMINAL_WRITE_CHARS)
    pendingTermWrites[id] = pending.slice(chunk.length)
    term.write(chunk, () => {
      if (pendingTermWrites[id] && termInstances[id]) scheduleTerminalFlush(id)
    })
  })
}

const writeTerminalData = (id, data) => {
  if (!termInstances[id] || !data) return

  pendingTermWrites[id] = (pendingTermWrites[id] || '') + data
  scheduleTerminalFlush(id)
}

const loadSessions = async () => {
  try {
    const res = await axios.get(`${API_BASE}/api/sessions`, authHeaders())
    if (res.data.success) {
      sessions.value = mergeSessionList(res.data.data)
      if (sessions.value.length > 0 && !activeSession.value) {
        activeSession.value = sessions.value[0].session_id
      } else if (activeSession.value && !sessions.value.some(s => s.session_id === activeSession.value)) {
        activeSession.value = sessions.value[0]?.session_id || null
      }
    }
  } catch (e) {}
}

const initTerminal = async (id) => {
  const el = termRefs[id]
  if (!el || termInstances[id]) return

  const term = new Terminal({
    cursorBlink: true,
    fontSize: 14,
    fontFamily: 'Menlo, Monaco, "Courier New", monospace',
    lineHeight: 1.35,
    scrollback: 2000,
    allowProposedApi: true,
    convertEol: false,
    linkHandler: {
      activate: (_event, text) => openSafeUrl(text),
      allowNonHttpProtocols: false,
    },
    theme: {
      background: 'transparent',
      foreground: '#f4f7fb',
      cursor: '#f4f7fb',
      selectionBackground: 'rgba(85, 111, 132, 0.5)',
      black: '#17212a',
      red: '#ff5f6d',
      green: '#66e85f',
      yellow: '#ffb44c',
      blue: '#48a8ff',
      magenta: '#9a6dff',
      cyan: '#58d9ff',
      white: '#dce6ee',
      brightBlack: '#586676',
      brightRed: '#ff7a86',
      brightGreen: '#82f36f',
      brightYellow: '#ffc464',
      brightBlue: '#6cbaff',
      brightMagenta: '#ad88ff',
      brightCyan: '#80e6ff',
      brightWhite: '#ffffff',
    },
  })

  const fitAddon = new FitAddon()
  configureTerminalUnicode(term)
  term.loadAddon(fitAddon)
  term.open(el)
  registerUrlLinks(term)

  termInstances[id] = term
  fitAddons[id] = fitAddon

  const token = sessionStorage.getItem('rmux_token') || ''
  const ws = new WebSocket(`${WS_BASE}/ws/terminal/${id}?token=${encodeURIComponent(token)}`)
  wsConnections[id] = ws

  ws.onopen = () => {
    setTimeout(() => {
      if (fitAddons[id]) {
        fitTerminal(id)
        if (ws.readyState === WebSocket.OPEN) {
          ws.send(JSON.stringify({ type: 'resize', cols: term.cols, rows: term.rows }))
        }
      }
      term.focus()
    }, 80)
  }

  ws.onmessage = (e) => {
    if (e.data === 'SESSION_NOT_FOUND' || e.data === 'SESSION_GONE') {
      term.write('\r\n\x1b[31m[会话已结束]\x1b[0m')
      return
    }
    writeTerminalData(id, e.data)
  }
  ws.onclose = () => {
    if (termInstances[id]) {
      term.write('\r\n\x1b[31m[连接已断开]\x1b[0m')
    }
  }

  term.onData((data) => {
    if (data === '\x16' && Date.now() < suppressCtrlVInputUntil) {
      return
    }
    sendTerminalMessage('input', data, id)
  })

  let resizeTimer = null
  term.onResize(({ cols, rows }) => {
    clearTimeout(resizeTimer)
    resizeTimer = setTimeout(() => {
      if (ws.readyState === WebSocket.OPEN) ws.send(JSON.stringify({ type: 'resize', cols, rows }))
    }, 220)
  })

  const termViewport = el.querySelector('.xterm-viewport') || el
  termViewport.addEventListener('contextmenu', async (e) => {
    e.preventDefault()
    if (term.hasSelection()) {
      await navigator.clipboard.writeText(term.getSelection())
      term.clearSelection()
      showToast('已复制')
    } else {
      await pasteClipboard(id)
    }
  })

  term.attachCustomKeyEventHandler((e) => {
    if (handlePasteShortcut(e, id)) {
      return false
    }
    if (e.type === 'keydown' && e.ctrlKey && e.key === 'Enter') {
      if (ws.readyState === WebSocket.OPEN) ws.send(JSON.stringify({ type: 'input', data: '\n' }))
      return false
    }
    return true
  })
}

const switchSession = async (id) => {
  activeSession.value = id
  await nextTick()
  if (!termInstances[id]) {
    await initTerminal(id)
  } else {
    setTimeout(() => {
      fitTerminal(id)
      termInstances[id]?.focus()
    }, 20)
  }
}

const createNewSession = async () => {
  try {
    let cols = 80
    let rows = 24
    if (termContainer.value) {
      const term = new Terminal({ fontSize: 14, fontFamily: 'Menlo, Monaco, "Courier New", monospace' })
      const fitAddon = new FitAddon()
      term.loadAddon(fitAddon)
      const dummy = document.createElement('div')
      dummy.style.visibility = 'hidden'
      dummy.style.position = 'absolute'
      dummy.style.width = '100%'
      dummy.style.height = '100%'
      termContainer.value.appendChild(dummy)
      term.open(dummy)
      fitAddon.fit()
      cols = term.cols
      rows = term.rows
      termContainer.value.removeChild(dummy)
      term.dispose()
    }

    const res = await axios.post(`${API_BASE}/api/sessions`, { type: 'local', cols, rows }, authHeaders())
    if (res.data.success) {
      const newSess = res.data.data
      appendSession(newSess)
      activeSession.value = newSess.session_id
      await nextTick()
      await initTerminal(newSess.session_id)
    }
  } catch (e) {
    showToast('创建失败: ' + (e.response?.data?.message || e.message))
  }
}

const closeSession = async (id) => {
  try {
    await axios.delete(`${API_BASE}/api/sessions/${id}`, authHeaders())
  } catch (e) {}
  if (wsConnections[id]) {
    wsConnections[id].close()
    delete wsConnections[id]
  }
  if (termInstances[id]) {
    termInstances[id].dispose()
    delete termInstances[id]
  }
  if (fitAddons[id]) delete fitAddons[id]
  if (writeFrameIds[id]) {
    cancelAnimationFrame(writeFrameIds[id])
    delete writeFrameIds[id]
  }
  delete pendingTermWrites[id]
  sessions.value = sessions.value.filter(s => s.session_id !== id)
  if (activeSession.value === id) activeSession.value = sessions.value[0]?.session_id || null
}

const beginRename = async (session) => {
  if (!session) return
  editingSession.value = session.session_id
  editingName.value = session.name || '本地终端'
  await nextTick()
  const input = renameInputRefs[session.session_id]
  input?.focus()
  input?.select()
}

const cancelRename = () => {
  editingSession.value = null
  editingName.value = ''
}

const commitRename = async (session) => {
  if (!session || editingSession.value !== session.session_id) return
  const currentName = session.name || '本地终端'
  const name = editingName.value.trim()
  editingSession.value = null
  editingName.value = ''

  if (!name || name === currentName) return

  try {
    const res = await axios.patch(`${API_BASE}/api/sessions/${session.session_id}/name`, { name }, authHeaders())
    if (res.data.success) {
      const updated = res.data.data
      const index = sessions.value.findIndex(s => s.session_id === session.session_id)
      if (index >= 0) sessions.value[index] = updated
      showToast('名称已更新')
    }
  } catch (e) {
    showToast(e.response?.data?.message || '重命名失败')
  }
}

const openSafeUrl = (raw) => {
  const url = String(raw || '').replace(/[),.;:!?]+$/g, '')
  if (!/^https?:\/\//i.test(url)) return
  window.open(url, '_blank', 'noopener,noreferrer')
}

const registerUrlLinks = (term) => {
  const urlPattern = /\bhttps?:\/\/[^\s<>"'`]+/gi
  term.registerLinkProvider({
    provideLinks(bufferLineNumber, callback) {
      const line = term.buffer.active.getLine(bufferLineNumber - 1)
      const text = line?.translateToString(true) || ''
      const links = []
      for (const match of text.matchAll(urlPattern)) {
        const raw = match[0]
        const url = raw.replace(/[),.;:!?]+$/g, '')
        if (!url) continue
        const start = (match.index || 0) + 1
        const end = start + url.length - 1
        links.push({
          range: {
            start: { x: start, y: bufferLineNumber },
            end: { x: end, y: bufferLineNumber },
          },
          text: url,
          decorations: { pointerCursor: true, underline: true },
          activate: (_event, value) => openSafeUrl(value),
        })
      }
      callback(links.length ? links : undefined)
    },
  })
}

const fitTerminal = (id) => {
  const term = termInstances[id]
  const fitAddon = fitAddons[id]
  if (!term || !fitAddon) return
  requestAnimationFrame(() => {
    try {
      fitAddon.fit()
      term.refresh(0, term.rows - 1)
    } catch (e) {}
  })
}

const copySelection = async () => {
  const term = termInstances[activeSession.value]
  if (!term || !term.hasSelection()) {
    showToast('没有选中文本')
    return
  }
  await navigator.clipboard.writeText(term.getSelection())
  term.clearSelection()
  showToast('已复制')
}

const pasteClipboard = async (sessionId = activeSession.value) => {
  try {
    if (navigator.clipboard?.read) {
      const items = await navigator.clipboard.read()
      for (const item of items) {
        const imageType = item.types.find(type => type.startsWith('image/'))
        if (imageType) {
          await pasteImageBlob(await item.getType(imageType), { sessionId })
          return
        }
      }
    }

    if (navigator.clipboard?.readText) {
      const text = await navigator.clipboard.readText()
      pasteText(text, { sessionId })
    }
  } catch (e) {
    showToast('粘贴失败: ' + (e.response?.data?.message || e.message || '请使用浏览器粘贴'))
  }
}

const pasteClipboardHistoryItem = (item) => {
  if (item.kind === 'image') {
    if (pasteImagePath(item.path, { recordItem: item, sessionId: activeSession.value })) clipboardOpen.value = false
    return
  }

  if (pasteText(item.text, { record: false, sessionId: activeSession.value })) {
    recordClipboardItem(item)
    clipboardOpen.value = false
  }
}

const openInBrowser = () => {
  const route = router.resolve('/terminal')
  const url = new URL(route.href, window.location.origin).toString()
  const opened = window.open(url, '_blank', 'noopener,noreferrer')
  if (!opened) showToast('浏览器阻止了新窗口')
}

let globalResizeTimer = null
let sessionRefreshTimer = null
const handleResize = () => {
  clearTimeout(globalResizeTimer)
  globalResizeTimer = setTimeout(() => {
    if (activeSession.value && fitAddons[activeSession.value]) fitTerminal(activeSession.value)
  }, 240)
}

onMounted(async () => {
  loadClipboardHistory()
  await Promise.all([loadSessions(), loadSystemInfo()])
  window.addEventListener('resize', handleResize)
  window.addEventListener('keydown', handleGlobalPasteKeydown, true)
  window.addEventListener('paste', handlePaste, true)
  sessionRefreshTimer = setInterval(loadSessions, SESSION_REFRESH_MS)
  if (activeSession.value) {
    await nextTick()
    await initTerminal(activeSession.value)
  }
})

onBeforeUnmount(() => {
  window.removeEventListener('resize', handleResize)
  window.removeEventListener('keydown', handleGlobalPasteKeydown, true)
  window.removeEventListener('paste', handlePaste, true)
  clearPendingShortcutPaste()
  clearInterval(sessionRefreshTimer)
  Object.values(writeFrameIds).forEach(id => cancelAnimationFrame(id))
  Object.values(wsConnections).forEach(ws => ws.close())
  Object.values(termInstances).forEach(t => t.dispose())
})

watch(activeSession, async (newVal) => {
  if (newVal) {
    await nextTick()
    if (!termInstances[newVal]) await initTerminal(newVal)
    else setTimeout(() => {
      fitTerminal(newVal)
      termInstances[newVal]?.focus()
    }, 20)
  }
})
</script>

<style scoped>
.terminal-shell {
  --bg: #071019;
  --panel: #0b151d;
  --panel-2: #101b24;
  --line: rgba(214, 231, 242, 0.12);
  --line-strong: rgba(214, 231, 242, 0.18);
  --text: #f4f7fb;
  --muted: #aab5c1;
  --dim: #657381;
  --green: #59df55;
  --blue: #3da7f7;
  --purple: #8d67f3;
  --orange: #ffae42;
  min-height: 100vh;
  display: grid;
  grid-template-columns: minmax(0, 1fr);
  color: var(--text);
  background:
    radial-gradient(circle at 82% 14%, rgba(55, 102, 130, 0.2), transparent 34%),
    linear-gradient(145deg, #040a10 0%, #0b1620 48%, #071019 100%);
  overflow: hidden;
  font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", system-ui, sans-serif;
}

button {
  font: inherit;
}

.icon-action,
.tab-rename,
.tab-close {
  border: 0;
  color: var(--text);
  background: transparent;
  cursor: pointer;
}

.icon-action:hover,
.tab-rename:hover,
.tab-close:hover,
.tool-btn:hover {
  background: rgba(255, 255, 255, 0.07);
}

.status-dot {
  width: 12px;
  height: 12px;
  flex: 0 0 auto;
  border-radius: 50%;
  display: inline-block;
  background: var(--dim);
  box-shadow: 0 0 18px currentColor;
}

.status-dot.online {
  color: var(--green);
  background: var(--green);
}

.status-dot.blue {
  color: var(--blue);
  background: var(--blue);
}

.status-dot.purple {
  color: var(--purple);
  background: var(--purple);
}

.status-dot.orange {
  color: var(--orange);
  background: var(--orange);
}

.terminal-panel {
  min-width: 0;
  min-height: 100vh;
  display: grid;
  grid-template-rows: 56px minmax(0, 1fr) 48px;
  background: rgba(6, 13, 19, 0.52);
}

.terminal-tabs {
  min-width: 0;
  display: flex;
  align-items: stretch;
  justify-content: space-between;
  border-bottom: 1px solid var(--line);
  background: rgba(7, 16, 23, 0.66);
}

.tabs-scroll {
  min-width: 0;
  display: flex;
  align-items: stretch;
  overflow-x: auto;
  overflow-y: hidden;
}

.terminal-tab {
  min-width: 0;
  max-width: 260px;
  height: 56px;
  display: flex;
  align-items: center;
  gap: 12px;
  padding: 0 16px;
  border: 0;
  border-right: 1px solid rgba(214, 231, 242, 0.08);
  color: #edf3f8;
  background: transparent;
  cursor: pointer;
  font-size: 16px;
}

.terminal-tab.active {
  background: rgba(255, 255, 255, 0.07);
}

.terminal-tab .tab-title {
  min-width: 0;
  max-width: 150px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.tab-name-input {
  width: min(150px, 36vw);
  min-width: 84px;
  height: 32px;
  padding: 0 8px;
  border: 1px solid rgba(72, 168, 255, 0.55);
  border-radius: 6px;
  outline: none;
  color: #edf3f8;
  background: rgba(4, 10, 16, 0.82);
  font: inherit;
  font-size: 16px;
}

.terminal-tab.add-tab {
  width: 52px;
  min-width: 52px;
  justify-content: center;
  font-size: 24px;
  color: #d6dee7;
}

.tab-rename,
.tab-close {
  flex: 0 0 28px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 28px;
  height: 28px;
  padding: 0;
  border-radius: 6px;
  color: var(--muted);
  font-size: 18px;
  line-height: 1;
}

.tab-close {
  font-size: 22px;
}

.toolbar {
  flex: 0 0 auto;
  display: flex;
  align-items: center;
  gap: 12px;
  padding: 0 16px;
}

.tool-btn,
.icon-action {
  height: 42px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  gap: 8px;
  border: 0;
  border-radius: 8px;
  color: #d6dee7;
  background: transparent;
  cursor: pointer;
  font-size: 16px;
}

.tool-btn {
  padding: 0 8px;
}

.tool-icon {
  color: #d6dee7;
  font-size: 19px;
}

.clipboard-menu {
  position: relative;
  display: inline-flex;
  align-items: center;
}

.clipboard-trigger {
  position: relative;
}

.history-count {
  min-width: 20px;
  height: 20px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  padding: 0 6px;
  border-radius: 999px;
  color: #06100c;
  background: var(--green);
  font-size: 12px;
  font-weight: 800;
  line-height: 1;
}

.clipboard-popover {
  position: absolute;
  top: calc(100% + 8px);
  right: 0;
  z-index: 20;
  width: min(420px, calc(100vw - 24px));
  max-height: min(520px, calc(100vh - 94px));
  display: grid;
  grid-template-rows: auto auto minmax(0, 1fr);
  border: 1px solid var(--line-strong);
  border-radius: 8px;
  background: rgba(8, 17, 25, 0.98);
  box-shadow: 0 24px 60px rgba(0, 0, 0, 0.42);
  overflow: hidden;
}

.clipboard-head {
  height: 44px;
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  padding: 0 14px;
  border-bottom: 1px solid var(--line);
  color: #edf3f8;
  font-weight: 700;
}

.clipboard-close {
  flex: 0 0 32px;
  width: 32px;
  height: 32px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  padding: 0;
  border: 0;
  border-radius: 6px;
  color: var(--muted);
  background: transparent;
  cursor: pointer;
  font-size: 22px;
  line-height: 1;
}

.clipboard-close:hover {
  color: var(--text);
  background: rgba(255, 255, 255, 0.08);
}

.clipboard-actions {
  display: grid;
  grid-template-columns: minmax(0, 1fr) auto;
  gap: 8px;
  padding: 10px;
  border-bottom: 1px solid var(--line);
}

.clipboard-actions button {
  height: 34px;
  border: 1px solid rgba(214, 231, 242, 0.14);
  border-radius: 7px;
  color: #dce6ee;
  background: rgba(255, 255, 255, 0.05);
  cursor: pointer;
  font-size: 13px;
}

.clipboard-actions button:disabled {
  color: var(--dim);
  cursor: default;
  opacity: 0.55;
}

.clipboard-actions button:not(:disabled):hover {
  background: rgba(255, 255, 255, 0.1);
}

.clipboard-empty {
  padding: 28px 16px;
  color: var(--muted);
  text-align: center;
  font-size: 14px;
}

.clipboard-list {
  min-height: 0;
  overflow: auto;
  padding: 6px;
}

.clipboard-item {
  width: 100%;
  min-height: 48px;
  display: grid;
  grid-template-columns: 44px minmax(0, 1fr) auto;
  align-items: center;
  gap: 10px;
  padding: 8px;
  border: 0;
  border-radius: 7px;
  color: #e8f0f6;
  background: transparent;
  cursor: pointer;
  text-align: left;
}

.clipboard-item:hover {
  background: rgba(255, 255, 255, 0.07);
}

.clipboard-kind {
  height: 24px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  border-radius: 6px;
  font-size: 12px;
  font-weight: 800;
}

.clipboard-kind.text {
  color: #d8edf6;
  background: rgba(72, 168, 255, 0.18);
}

.clipboard-kind.image {
  color: #ecf7df;
  background: rgba(89, 223, 85, 0.18);
}

.clipboard-preview {
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  color: #edf3f8;
  font-size: 14px;
}

.clipboard-time {
  color: var(--dim);
  font-family: Menlo, Monaco, "Courier New", monospace;
  font-size: 12px;
}

.icon-action {
  width: 42px;
  font-size: 24px;
}

.terminal-viewport {
  position: relative;
  min-width: 0;
  min-height: 0;
  padding: 24px 28px;
  background:
    radial-gradient(circle at 50% 0, rgba(42, 82, 104, 0.12), transparent 38%),
    rgba(2, 8, 13, 0.58);
}

.term-wrapper {
  position: absolute;
  inset: 24px 28px;
  display: block;
  visibility: hidden;
  pointer-events: none;
}

.term-wrapper.active {
  visibility: visible;
  pointer-events: auto;
}

:deep(.xterm) {
  height: 100%;
  padding: 0;
}

:deep(.xterm-viewport) {
  background: transparent !important;
  scrollbar-width: thin;
  scrollbar-color: rgba(255, 255, 255, 0.18) transparent;
}

:deep(.xterm-viewport)::-webkit-scrollbar {
  width: 6px;
  height: 6px;
}

:deep(.xterm-viewport)::-webkit-scrollbar-track {
  background: transparent;
}

:deep(.xterm-viewport)::-webkit-scrollbar-thumb {
  background: rgba(255, 255, 255, 0.14);
  border-radius: 3px;
  transition: background 0.2s ease;
}

:deep(.xterm-viewport):hover::-webkit-scrollbar-thumb {
  background: rgba(255, 255, 255, 0.28);
}

:deep(.xterm-viewport)::-webkit-scrollbar-thumb:hover {
  background: rgba(255, 255, 255, 0.42);
}

:deep(.xterm-viewport)::-webkit-scrollbar-button {
  display: none;
}

:deep(.xterm-viewport)::-webkit-scrollbar-corner {
  background: transparent;
}

:deep(.xterm-screen) {
  min-height: 100%;
}

.empty-state {
  height: 100%;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 14px;
  color: var(--muted);
  font-family: Menlo, Monaco, "Courier New", monospace;
}

.empty-title {
  color: #f4f7fb;
  font-size: 24px;
}

.empty-line {
  font-size: 14px;
}

.primary-command {
  margin-top: 8px;
  padding: 11px 16px;
  border: 0;
  border-radius: 8px;
  color: #041008;
  background: var(--green);
  cursor: pointer;
  font-weight: 800;
}

.statusbar {
  display: grid;
  grid-template-columns: minmax(0, 1fr) auto minmax(0, 1fr);
  align-items: center;
  column-gap: 20px;
  padding: 0 28px;
  border-top: 1px solid var(--line);
  color: #d6dee7;
  background: rgba(7, 16, 23, 0.7);
  font-size: 16px;
}

.status-left,
.status-center,
.status-right {
  display: flex;
  align-items: center;
  gap: 14px;
  min-width: 0;
}

.status-left {
  justify-self: start;
  min-width: 0;
  overflow: hidden;
}

.status-center {
  justify-content: center;
  color: #a9bac8;
  font-size: 14px;
  justify-self: center;
  min-width: 0;
  white-space: nowrap;
}

.repo-label {
  color: #a9bac8;
}

.status-center a {
  color: #79d96b;
  text-decoration: none;
}

.status-center a:hover {
  text-decoration: underline;
}

.status-right {
  justify-self: end;
  min-width: 0;
  color: #c2cad3;
  font-family: Menlo, Monaco, "Courier New", monospace;
}

.status-path {
  color: #9fbdce;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  max-width: min(44vw, 520px);
}

.paste-toast {
  position: fixed;
  left: 50%;
  bottom: 34px;
  transform: translateX(-50%);
  z-index: 10;
  padding: 10px 16px;
  border: 1px solid var(--line-strong);
  border-radius: 8px;
  color: #f4f7fb;
  background: rgba(9, 18, 26, 0.94);
  box-shadow: 0 18px 44px rgba(0, 0, 0, 0.32);
}

@media (max-width: 980px) {
  .terminal-tabs {
    flex-direction: column;
    height: auto;
  }

  .terminal-panel {
    grid-template-rows: auto minmax(0, 1fr) 44px;
  }

  .toolbar {
    height: 50px;
    padding: 0 12px;
    border-top: 1px solid rgba(214, 231, 242, 0.08);
    overflow-x: auto;
  }

  .terminal-tab {
    height: 48px;
    font-size: 15px;
  }

  .tool-btn > span:not(.tool-icon):not(.history-count) {
    display: none;
  }

  .clipboard-popover {
    position: fixed;
    top: 104px;
    right: 12px;
    width: calc(100vw - 24px);
    max-height: calc(100vh - 160px);
  }

  .statusbar {
    padding: 0 14px;
    font-size: 12px;
    column-gap: 10px;
  }

  .status-right span:first-child {
    display: none;
  }

  .status-center {
    display: none;
  }

  .status-path {
    max-width: 42vw;
  }
}

@media (max-width: 640px) {
  .terminal-viewport {
    padding: 14px;
  }

  .term-wrapper {
    inset: 14px;
  }
}
</style>
