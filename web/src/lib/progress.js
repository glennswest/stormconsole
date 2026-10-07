// An operation, node by node (#84): the owner's "cluster progress screen as
// they reconfigure" (stormcluster#1).
//
// stormcluster's `GET /api/v1/operations/{id}` is one list of steps, each
// naming its node when it has one (`step: {step: "enroll", node: "b2"}`).
// Read node by node it answers "where is each machine": b1 done, b2 failed
// at its join token, b3 not started. Pure, so it is tested with plain node
// (progress.test.mjs) and the page only draws it.

/// The nodes a request names, in the order it names them.
export function requestNodes(req) {
  if (!req || typeof req !== 'object') return []
  const out = []
  const add = (n) => n && !out.includes(n) && out.push(n)
  for (const k of ['masters', 'workers', 'nodes']) for (const n of req[k] || []) add(n)
  add(req.node)
  return out
}

const RANK = { failed: 3, running: 2, pending: 1, done: 0 }

/// One row per node, then one for the steps that name no node ("the
/// cluster": the endpoint, publishing the record). Each row: `node`,
/// `status` (failed > running > pending > done, so a node is only done when
/// every step of it is), `current` (the step it is at, or failed in),
/// `done`/`total`, and its `steps`.
export function byNode(op) {
  const steps = op?.steps || []
  const order = requestNodes(op?.request)
  const groups = new Map()
  const group = (key) => {
    if (!groups.has(key)) groups.set(key, [])
    return groups.get(key)
  }
  for (const n of order) group(n)
  steps.forEach((s, i) => {
    const node = s?.step?.node || ''
    if (node && !order.includes(node)) order.push(node)
    group(node).push({ ...s, index: i, status: s.status || 'pending' })
  })
  const keys = [...order, ...(groups.has('') ? [''] : [])]
  return keys.map((key) => {
    const ss = groups.get(key) || []
    const total = ss.length
    const done = ss.filter((s) => s.status === 'done').length
    let status = total === 0 ? (op?.state === 'done' ? 'done' : 'pending') : 'done'
    for (const s of ss) if (RANK[s.status] > RANK[status]) status = s.status
    const current =
      ss.find((s) => s.status === 'failed') || ss.find((s) => s.status === 'running') || ss.find((s) => s.status === 'pending') || null
    return { node: key || null, label: key || 'the cluster', status, current, done, total, steps: ss }
  })
}

/// Whether an operation is still moving (worth polling).
export const moving = (op) => !!op && op.state === 'running'
