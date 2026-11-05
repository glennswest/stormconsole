// What the Update page makes of stormupdate's status (#164), apart from the page so
// plain node can test it.

// A node's walk, in order. PrePulling happens before any node moves; a node that
// fails leaves the walk for the rollback's.
export const STEPS = ['PrePulling', 'Pending', 'Draining', 'Upgrading', 'Verifying', 'Done']
export const STEP_WORDS = {
  PrePulling: 'Pulling images',
  Pending: 'Waiting its turn',
  Draining: 'Draining',
  Upgrading: 'Staging and rebooting',
  Verifying: 'Checking it came back',
  Done: 'Done',
  RollingBack: 'Rolling back',
  RolledBack: 'Rolled back',
  Failed: 'Failed',
}

/// A node's steps, each done, here or still to come. Off the walk (a failure), the
/// steps it had not reached stay undone and the end says what became of it.
export function nodeSteps(n) {
  const at = STEPS.indexOf(n.phase)
  const steps = STEPS.map((s, i) => ({ step: s, word: STEP_WORDS[s], state: at < 0 ? 'off' : i < at ? 'done' : i === at ? (s === 'Done' ? 'done' : 'now') : 'todo' }))
  if (at < 0 && n.phase) steps.push({ step: n.phase, word: STEP_WORDS[n.phase] || n.phase, state: n.phase === 'RolledBack' ? 'warn' : n.phase === 'Failed' ? 'bad' : 'now' })
  return steps
}

/// The one line for what a node is doing: the agent's words when it has said any,
/// which name the volumes and the release; then the gates it waits on; then the
/// controller's.
export function nodeLine(n) {
  if (n.phase === 'Verifying' && n.waitingOn?.length) return `waiting on ${n.waitingOn.join(', ')}`
  return n.agentMessage || n.message || ''
}

export function duration(s) {
  if (s == null || s < 0) return '—'
  if (s < 60) return `${s}s`
  const m = Math.floor(s / 60)
  if (m < 60) return `${m}m ${s % 60}s`
  const h = Math.floor(m / 60)
  if (h < 24) return `${h}h ${m % 60}m`
  return `${Math.floor(h / 24)}d ${h % 24}h`
}

export const running = (phase) => ['Progressing', 'Paused', 'RollingBack'].includes(phase)

/// What is on offer to do, with why not where it is not.
export function actions(spec, st) {
  const phase = st?.phase || 'Idle'
  const moving = running(phase)
  return {
    start: !moving,
    pause: phase === 'Progressing' && !spec?.paused,
    resume: !!spec?.paused || phase === 'Paused',
    rollback: !!st?.previousRelease && phase !== 'RollingBack' && !st?.rollbackRun,
  }
}

/// The releases in view: running, staged here, the target, and the one rolled back from.
export function releases(st, generations) {
  const nodes = st?.nodes || []
  const versions = [...new Set(nodes.map((n) => n.release).filter(Boolean))]
  const lastBack = [...(st?.history || [])].reverse().find((h) => h.result === 'RolledBack')
  const back = st?.phase === 'RolledBack' || st?.phase === 'RollingBack' || st?.rollbackRun
  return {
    running: st?.currentRelease || (versions.length ? versions.join(', ') : ''),
    mixed: !st?.currentRelease && versions.length > 1,
    staged: generations?.staged?.version || '',
    stagedComplete: !!generations?.staged?.complete,
    target: running(st?.phase) ? st?.targetRelease || '' : '',
    previous: st?.previousRelease || '',
    rolledBackFrom: back ? st?.targetRelease || lastBack?.to || '' : lastBack?.to || '',
    rolledBackFromNow: !!back,
  }
}

/// History newest first, each with how long it took and a tone.
export function historyRows(st) {
  return [...(st?.history || [])].reverse().map((h) => ({
    ...h,
    took: h.finished ? h.finished - h.started : null,
    tone: h.result === 'Completed' ? 'ok' : h.result === 'Progressing' ? 'idle' : h.result === 'RolledBack' ? 'warn' : 'error',
  }))
}

/// How far the target's images have come, per image: the nodes in each state.
export function imageRows(st) {
  return (st?.images || []).map((i) => {
    const counts = {}
    for (const n of i.nodes || []) counts[n.state] = (counts[n.state] || 0) + 1
    return {
      name: i.name,
      digest: i.digest,
      policy: i.dataPolicy,
      states: Object.entries(counts).map(([state, count]) => ({ state, count })),
      failed: (i.nodes || []).filter((n) => n.state === 'Failed').map((n) => `${n.node}: ${n.message || 'failed'}`),
    }
  })
}

/// A change between releases, in a line.
export function changeLine(c) {
  const short = (s) => (s && s.length > 16 ? s.slice(0, 12) : s)
  const what = c.kind ? `${c.component} (${c.kind})` : c.component
  if (c.from && c.to) return `${what} ${short(c.from)} → ${short(c.to)}`
  return c.to ? `${what} added` : `${what} removed`
}
