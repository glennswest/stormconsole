// REST + WebSocket helpers. All server communication goes through here.

export async function get(path) {
  const resp = await fetch(path)
  if (!resp.ok) throw new Error(`${resp.status} ${resp.statusText}`)
  return resp.json()
}

export async function post(path) {
  return call('POST', path)
}

/// Invoke a component action exactly as the feed declares it — a
/// stormblock delete is a DELETE, a stormd restart a POST.
///
/// Destructive storage (format, sanitize, wipe, RAID sets, slabs, forge,
/// volume deletes) answers 428 naming what to type (#82): the person types
/// the drive's serial, or the object's name, and the request goes again
/// carrying it. The server decides which actions those are and checks the
/// word, so every button that comes through here gets the same question.
export async function call(method, path, confirmed) {
  // A path that is a route opens it, rather than being fetched.
  //
  // Some actions are "go and look at this" -- a serial console, a screen --
  // and a row had no way to offer one: every action was a request, so the
  // only way to reach a machine's console was to know the URL. Fetching
  // `#/vm/default/web-1` asks the server for a document that does not exist
  // and fails in a way that looks like the machine refused.
  if (typeof path === 'string' && path.startsWith('#/')) {
    window.location.hash = path.slice(1)
    return {}
  }
  const headers = confirmed === undefined ? {} : { 'X-Storm-Confirm': confirmed }
  const resp = await fetch(path, { method: method || 'POST', headers })
  if (!resp.ok) {
    const data = await resp.json().catch(() => ({}))
    if (resp.status === 428 && data.guard && confirmed === undefined) {
      return call(method, path, typedConfirm(data.guard))
    }
    throw new Error(data.error || `${resp.status} ${resp.statusText}`)
  }
  return resp.json().catch(() => ({}))
}

/// Ask for the object's name, typed. Stronger than OK: it has to be read
/// off the screen, so it cannot be clicked through.
function typedConfirm(g) {
  const typed = window.prompt(
    `${g.what[0].toUpperCase()}${g.what.slice(1)}: ${g.confirm}\n\n` +
      `This destroys data and cannot be undone. It is done as you, and ` +
      `only a storage-admin may.\n\nType ${g.confirm} to confirm.`,
  )
  if (typed === null) throw new Error('cancelled')
  if (typed.trim() !== g.confirm) throw new Error(`not confirmed: that is not ${g.confirm}`)
  return typed.trim()
}

/// A JSON body with a method. PUT is a replace or a patch and POST is a
/// create; the caller knows which it is doing, and defaulting to POST
/// meant every PUT route grew a POST alias to be reachable from here.
export async function postJson(path, body, method = 'POST') {
  const resp = await fetch(path, {
    method,
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body),
  })
  const data = await resp.json().catch(() => ({}))
  if (!resp.ok) throw new Error(data.error || `${resp.status} ${resp.statusText}`)
  return data
}

export function wsUrl(path) {
  const proto = location.protocol === 'https:' ? 'wss:' : 'ws:'
  return `${proto}//${location.host}${path}`
}

/// A WebSocket that redials itself. onmessage receives parsed JSON,
/// onstatus receives 'connecting' | 'open' | 'closed'.
export function reconnectingSocket(path, { onmessage, onstatus } = {}) {
  let ws = null
  let closed = false
  let delay = 500

  function dial() {
    if (closed) return
    onstatus?.('connecting')
    ws = new WebSocket(wsUrl(path))
    ws.onopen = () => {
      delay = 500
      onstatus?.('open')
    }
    ws.onmessage = (e) => {
      try {
        onmessage?.(JSON.parse(e.data))
      } catch {
        /* non-JSON frame — ignore */
      }
    }
    ws.onclose = () => {
      onstatus?.('closed')
      if (!closed) {
        setTimeout(dial, delay)
        delay = Math.min(delay * 2, 10000)
      }
    }
  }

  dial()
  return {
    close() {
      closed = true
      ws?.close()
    },
  }
}

export { formatBytes, formatDuration, timeAgo, escapeHtml, ansiToHtml } from 'stormview/utils'
