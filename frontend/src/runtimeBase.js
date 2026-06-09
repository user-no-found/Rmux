const APP_PREFIX = '/app/rmux'
const BUILD_VERSION = import.meta.env.VITE_APP_BUILD_VERSION || ''

const path = window.location.pathname
const hasAppPrefix = path === APP_PREFIX || path.startsWith(`${APP_PREFIX}/`)

export const APP_BASE = hasAppPrefix ? `${APP_PREFIX}/` : '/'
export const API_BASE = hasAppPrefix ? APP_PREFIX : ''
export const WS_BASE = `${location.origin.replace(/^http/, 'ws')}${hasAppPrefix ? APP_PREFIX : ''}`
export const assetUrl = (path) => {
  const cleanPath = path.replace(/^\/+/, '')
  if (!BUILD_VERSION || cleanPath.includes('?')) {
    return `${APP_BASE}${cleanPath}`
  }
  return `${APP_BASE}${cleanPath}?v=${encodeURIComponent(BUILD_VERSION)}`
}
