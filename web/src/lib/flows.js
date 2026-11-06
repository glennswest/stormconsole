// What the Network Flows view makes of a flow (#4), kept apart so plain node can test it.

const WORDS = { FORWARDED: 'allowed', DROPPED: 'dropped', AUDIT: 'audit', REDIRECTED: 'redirected', ERROR: 'error', TRACED: 'traced', TRANSLATED: 'translated' }

export const VERDICTS = [['', 'Every verdict'], ['FORWARDED', 'Allowed'], ['DROPPED', 'Dropped'], ['ERROR', 'Error']]

export function verdictWord(v) {
  return WORDS[v] || (v || 'unknown').toLowerCase()
}

export const verdictTone = (v) => (v === 'DROPPED' || v === 'ERROR' ? 'bad' : v === 'FORWARDED' ? 'ok' : 'dim')

/// A drop's reason in words: POLICY_DENIED is "policy denied".
export function dropWord(d) {
  return d ? d.toLowerCase().replaceAll('_', ' ') : ''
}

/// One end: a project and pod, what the network calls it, or that it is someone else's.
export function endWord(e) {
  if (!e) return '—'
  if (e.hidden) return 'another project'
  if (e.pod) return `${e.project}/${e.pod}`
  const reserved = (e.labels || []).find((l) => l.startsWith('reserved:'))
  if (reserved) return `${reserved.slice('reserved:'.length)}${e.ip ? ` ${e.ip}` : ''}`
  return e.ip || 'unknown'
}

export function portWord(f) {
  if (!f.protocol) return ''
  return f.port ? `${f.protocol}/${f.port}` : f.protocol
}

/// Newest first, kept to a size a table can hold while following.
export function addFlow(rows, flow, keep = 300) {
  return [flow, ...rows].slice(0, keep)
}

/// The query both the list and the stream take.
export function flowQuery({ ns, pod, verdict, last }) {
  const q = new URLSearchParams()
  if (ns) q.set('ns', ns)
  if (pod) q.set('pod', pod)
  if (verdict) q.set('verdict', verdict)
  if (last) q.set('last', last)
  return q.toString()
}
