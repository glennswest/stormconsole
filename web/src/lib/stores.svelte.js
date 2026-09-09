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
// namespaces.
//
// It is a *dimension*, not a menu item (#5): it scopes every namespaced
// view, it says nothing about the cluster-scoped ones, and it travels in
// the URL — so a link somebody pastes shows what they were looking at,
// which a selector kept only in localStorage cannot do. The stored value
// is the default for a fresh tab; the URL wins wherever it says anything.
export const k8sns = $state({ selected: readStored() })

function readStored() {
  try {
    return localStorage.getItem('stormconsole-ns') || ''
  } catch {
    return ''
  }
}

/// Which hash routes are scoped by the selector. A cluster-scoped kind is
/// not one of them and says so on its own page; the kind catalogue is the
/// authority (see `kinds`).
export function routeTakesNamespace(hash = location.hash) {
  const path = (hash || '#/').split('?')[0]
  if (path === '#/k8s/events' || path === '#/vms') return true
  if (path.startsWith('#/k8s/') && !path.startsWith('#/k8s/ns/')) {
    return isNamespaced(path.slice('#/k8s/'.length))
  }
  return false
}

/// Rewrite the current hash so it carries the selection. `replace` keeps
/// the back button meaning what it did before the selector was touched.
function writeNamespaceToUrl(ns, { replace = false } = {}) {
  const [path, query] = (location.hash || '#/').split('?')
  const q = new URLSearchParams(query || '')
  if (ns) q.set('ns', ns)
  else q.delete('ns')
  const next = q.toString() ? `${path}?${q}` : path
  if (next === (location.hash || '#/')) return
  if (replace) history.replaceState(null, '', next)
  else location.hash = next
}

export function selectNamespace(ns) {
  k8sns.selected = ns
  try {
    localStorage.setItem('stormconsole-ns', ns)
  } catch {}
  if (routeTakesNamespace()) writeNamespaceToUrl(ns)
}

/// Keep the selector and the address bar agreeing. Called on every hash
/// change: a URL that names a namespace sets the selection, and a scoped
/// route that does not carries the current one so the address is always a
/// complete description of what is on screen.
export function syncNamespaceWithUrl() {
  if (!routeTakesNamespace()) return
  const q = new URLSearchParams((location.hash || '').split('?')[1] || '')
  if (q.has('ns')) {
    const ns = q.get('ns') || ''
    if (ns !== k8sns.selected) {
      k8sns.selected = ns
      try {
        localStorage.setItem('stormconsole-ns', ns)
      } catch {}
    }
    return
  }
  if (k8sns.selected) writeNamespaceToUrl(k8sns.selected, { replace: true })
}

window.addEventListener('hashchange', syncNamespaceWithUrl)

// The kind catalogue, from the plugin that owns the kinds
// (/api/plugins/k8s/kinds). What a kind is called and whether the
// namespace selector applies to it used to be written down three times in
// this app; it is declared once, beside the watch that produces the
// objects.
export const kinds = $state({ list: [], loaded: false })

export function kindSpec(kind) {
  return kinds.list.find((k) => k.kind === kind) || null
}

export function kindTitle(kind) {
  return kindSpec(kind)?.title || kind
}

export function isNamespaced(kind) {
  return !!kindSpec(kind)?.namespaced
}

// What this viewer is not being shown, and whether anything is being
// enforced at all (/api/v1/console/access). A short list with no
// explanation reads as a broken console.
export const access = $state({ enforced: false, identified: false, hidden: 0, plugins: {} })

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

  get('/api/plugins/k8s/kinds')
    .then((list) => {
      kinds.list = Array.isArray(list) ? list : []
      kinds.loaded = true
      // The catalogue decides which routes are scoped, so the URL can
      // only be reconciled once it is here.
      syncNamespaceWithUrl()
    })
    .catch(() => { kinds.loaded = true })

  get('/api/v1/console/access')
    .then((a) => Object.assign(access, a))
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

// The two chrome styles. Orthogonal to stormview's themes: a theme is
// the palette, a style is the proportion and structure. Both work on all
// twelve palettes, so picking one never constrains the other.
export const STYLES = [
  { id: 'openshift', label: 'OpenShift' },
  { id: 'esxi', label: 'ESXi' },
]

export const prefs = $state({
  view: load('stormconsole-view', 'table'),
  collapsed: load('stormconsole-nav-collapsed', {}),
  style: load('stormconsole-style', 'openshift'),
  navOpen: true,
})

export function setView(v) {
  prefs.view = v
  save('stormconsole-view', v)
}

export function applyStyle(id) {
  const style = STYLES.some((s) => s.id === id) ? id : 'openshift'
  prefs.style = style
  document.documentElement.setAttribute('data-style', style)
  save('stormconsole-style', style)
}

/// Called before the app mounts so the first paint is already in the
/// chosen style — no flash of the default chrome.
export function initStyle() {
  applyStyle(load('stormconsole-style', 'openshift'))
}

export function toggleSection(label) {
  prefs.collapsed = { ...prefs.collapsed, [label]: !prefs.collapsed[label] }
  save('stormconsole-nav-collapsed', prefs.collapsed)
}

// --- Feed helpers -----------------------------------------------------

/// Every component under one id prefix, sorted.
function withPrefix(prefix) {
  return feed.components.filter((c) => c.id.startsWith(prefix)).map((c) => c.id).sort()
}

/// The ids a route shows, so a view and its nav badge always agree on the
/// count. `#/k8s/<kind>` is the kind's slice of the feed, scoped by the
/// namespace selector when the catalogue says the kind is namespaced;
/// `#/grid?id=…&rel=…` is that relationship's targets.
export function idsForRoute(href) {
  if (!href) return null
  const [path, query] = href.split('?')
  const q = new URLSearchParams(query || '')
  // A link's own ?ns= wins over the selector, so a count in the navigator
  // and the page it leads to always agree.
  const ns = q.has('ns') ? q.get('ns') : k8sns.selected

  // The two hardware routes count different things: the shelves page is
  // a list of enclosures, not of the disks in them.
  if (path === '#/drives') {
    return q.get('group') === 'shelf' ? withPrefix('drive:shelf:') : withPrefix('drive:drive:')
  }
  if (path === '#/vms') {
    let ids = withPrefix('vm:')
    if (ns) ids = ids.filter((id) => id.split(':')[2]?.startsWith(`${ns}/`))
    return ids
  }

  if (path.startsWith('#/k8s/ns/')) return null

  if (path.startsWith('#/k8s/') && path !== '#/k8s/events') {
    const kind = path.slice('#/k8s/'.length)
    const prefix = `k8s:${kind}:`
    let ids = withPrefix(prefix)
    if (ns && isNamespaced(kind)) {
      ids = ids.filter((id) => id.startsWith(`${prefix}${ns}/`))
    }
    return ids
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
