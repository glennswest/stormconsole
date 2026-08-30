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
export async function call(method, path) {
  const resp = await fetch(path, { method: method || 'POST' })
  if (!resp.ok) {
    const data = await resp.json().catch(() => ({}))
    throw new Error(data.error || `${resp.status} ${resp.statusText}`)
  }
  return resp.json().catch(() => ({}))
}

export async function postJson(path, body) {
  const resp = await fetch(path, {
    method: 'POST',
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
