// Shared live state: the aggregated component feed, the server-assembled
// navigation, and the auth gate. The feed arrives over /ws/components as
// full snapshots (with a REST fetch for the first paint), so every
// consumer always holds a complete picture and there is no client-side
// merging.

import { get, postJson, reconnectingSocket } from './api.js'
import { setDefaultTheme } from 'stormview/theme'

export const feed = $state({
  components: [],
  connected: false,
  loaded: false,
})

// Navigation comes from the server (/api/v1/console/nav) — plugins declare
// it, the host merges it, this app renders whatever it is given.
export const nav = $state({
  name: 'stormconsole',
  sections: [],
})

export const auth = $state({
  checked: false,
  required: false,
  authenticated: true,
  user: null,
})

export async function checkAuth() {
  try {
    const s = await get('/api/v1/auth/session')
    auth.required = !!s.required
    auth.authenticated = !!s.authenticated
    auth.user = s.user || null
    if (s.container) nav.name = s.container
    if (s.theme) setDefaultTheme(s.theme)
  } catch {
    // Can't tell — let the app try; data requests will 401 if auth is on.
  }
  auth.checked = true
}

export async function login(username, password) {
  const r = await postJson('/api/v1/auth/login', { username, password })
  auth.authenticated = true
  auth.user = r.user || username || null
  startFeed()
}

export async function logout() {
  try {
    await postJson('/api/v1/auth/logout', {})
  } catch {}
  location.reload()
}

let started = false

export function startFeed() {
  if (started) return
  started = true

  get('/api/v1/components')
    .then((list) => {
      if (!feed.loaded) {
        feed.components = list
        feed.loaded = true
      }
    })
    .catch(() => {})

  reconnectingSocket('/ws/components', {
    onmessage(list) {
      feed.components = list
      feed.loaded = true
    },
    onstatus(s) {
      feed.connected = s === 'open'
    },
  })

  get('/api/v1/console/nav')
    .then((sections) => { nav.sections = sections })
    .catch(() => {})
}
