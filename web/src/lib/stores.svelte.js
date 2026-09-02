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

// The namespace selector, OpenShift's project selector: '' means all
// namespaces. Scopes every namespaced k8s view; persists per browser.
export const k8sns = $state({
  selected: (() => {
    try {
      return localStorage.getItem('stormconsole-ns') || ''
    } catch {
      return ''
    }
  })(),
})

export function selectNamespace(ns) {
  k8sns.selected = ns
  try {
    localStorage.setItem('stormconsole-ns', ns)
  } catch {}
}

// What can be created, declared by plugins (/api/v1/console/creators):
// a YAML editor with a template or a form, each posting to a plugin path.
export const creators = $state({ list: [] })

// The open create dialog, if any.
export const create = $state({ open: null })

export function openCreator(c) {
  create.open = c
}

export function closeCreator() {
  create.open = null
}

/// Creators offered on a hash route: those declared for it (prefix match)
/// or everywhere ("*"). `null` means all of them (the top-bar menu).
export function creatorsFor(hash) {
  if (hash === null || hash === undefined) return creators.list
  return creators.list.filter(
    (c) => (c.at || []).includes('*') || (c.at || []).some((a) => hash.startsWith(a))
  )
}

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

  get('/api/v1/console/creators')
    .then((list) => { creators.list = list })
    .catch(() => {})
}

// --- View preferences -------------------------------------------------
// How a list is drawn (dense table or cards) and which navigator groups
// are collapsed are the operator's choice, not the app's, so they persist
// per browser like the namespace selector does.

function load(key, fallback) {
  try {
    const v = localStorage.getItem(key)
    return v === null ? fallback : JSON.parse(v)
  } catch {
    return fallback
  }
}

function save(key, value) {
  try {
    localStorage.setItem(key, JSON.stringify(value))
  } catch {}
}

export const prefs = $state({
  view: load('stormconsole-view', 'table'),
  collapsed: load('stormconsole-nav-collapsed', {}),
  navOpen: true,
})

export function setView(v) {
  prefs.view = v
  save('stormconsole-view', v)
}

export function toggleSection(label) {
  prefs.collapsed = { ...prefs.collapsed, [label]: !prefs.collapsed[label] }
  save('stormconsole-nav-collapsed', prefs.collapsed)
}

// --- Feed helpers -----------------------------------------------------

const NAMESPACED = ['pod', 'deploy', 'sts', 'ds', 'job', 'cronjob', 'svc', 'pvc', 'netpol', 'cnp', 'cep']

/// The ids a route shows, so a view and its nav badge always agree on the
/// count. `#/k8s/<kind>` is the kind's slice of the feed, scoped by the
/// namespace selector; `#/grid?id=…&rel=…` is that relationship's targets.
export function idsForRoute(href) {
  if (!href) return null
  const [path, query] = href.split('?')
  const q = new URLSearchParams(query || '')

  if (path.startsWith('#/k8s/') && path !== '#/k8s/events') {
    const kind = path.slice('#/k8s/'.length)
    const prefix = `k8s:${kind}:`
    let ids = feed.components.filter((c) => c.id.startsWith(prefix)).map((c) => c.id)
    if (k8sns.selected && NAMESPACED.includes(kind)) {
      ids = ids.filter((id) => id.startsWith(`${prefix}${k8sns.selected}/`))
    }
    return ids.sort()
  }

  if (path === '#/grid' && q.get('id')) {
    const root = feed.components.find((c) => c.id === q.get('id'))
    if (!root) return null
    const rel = q.get('rel')
    if (!rel) return [root.id]
    const r = (root.relations || []).find((x) => x.name === rel)
    return r ? r.targets : []
  }

  return null
}

/// The badge beside a nav item, or null when the route has no countable
/// contents (the overview, the log tail).
export function navCount(href) {
  const ids = idsForRoute(href)
  return ids ? ids.length : null
}

/// How the whole feed is doing, for the masthead pill and the overview
/// summary. Plugin cards are excluded — they report their own children.
export function rollup(components = feed.components) {
  const r = { ok: 0, warn: 0, error: 0, idle: 0, unknown: 0, total: 0 }
  for (const c of components) {
    if (c.kind === 'plugin') continue
    r.total++
    r[c.health] = (r[c.health] ?? 0) + 1
  }
  return r
}
