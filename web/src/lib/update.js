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

/// A node's steps, each done, here or still to come. Off the walk (a failure, a
/// rollback) the walk says nothing true any more, so only where it ended is shown.
export function nodeSteps(n) {
  // A pull elsewhere stopped the run: this node is not waiting for anything (stormupdate rollout.rs).
  if (n.phase === 'Pending' && n.message === 'pre-pull stopped') return [{ step: 'Stopped', word: 'Stopped', state: 'warn' }]
  const at = STEPS.indexOf(n.phase)
  if (at < 0) {
    if (!n.phase) return []
    return [{ step: n.phase, word: STEP_WORDS[n.phase] || n.phase, state: n.phase === 'RolledBack' ? 'warn' : n.phase === 'Failed' ? 'bad' : 'now' }]
  }
  return STEPS.map((s, i) => ({ step: s, word: STEP_WORDS[s], state: i < at ? 'done' : i === at ? (s === 'Done' ? 'done' : 'now') : 'todo' }))
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
    // A pull that stopped the run moved nothing, so there is nothing to roll back.
    rollback: !!st?.previousRelease && phase !== 'RollingBack' && !st?.rollbackRun && prePull(st)?.state !== 'stopped',
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

const IMAGE_STATE_WORDS = {
  InRelease: 'In the release',
  PreUpgrade: 'Pre-upgrade hook',
  PostUpgrade: 'Post-upgrade hook',
  RolledBack: 'Rolled back',
}
const imageState = (s) => IMAGE_STATE_WORDS[s] || s

/// What happens to an image's data, in stormupdate's terms (its README, Images).
const POLICY_WORDS = {
  reuse: 'kept as it is',
  replace: 'a fresh volume; the old one is kept for rollback',
  migrate: 'migrated on a copy; a rollback boots on the untouched original',
}

/// The target's images: each one's identity, its data, its hooks, and each node's state.
export function imageRows(st) {
  return (st?.images || []).map((i) => {
    const counts = {}
    for (const n of i.nodes || []) {
      const word = imageState(n.state)
      counts[word] = (counts[word] || 0) + 1
    }
    return {
      name: i.name,
      digest: i.digest,
      reference: i.reference || '',
      volume: i.dataVolume || '',
      policy: i.dataPolicy || 'reuse',
      policyWords: POLICY_WORDS[i.dataPolicy || 'reuse'] || '',
      hooks: i.hooks || [],
      states: Object.entries(counts).map(([state, count]) => ({ state, count })),
      nodes: (i.nodes || []).map((n) => ({ node: n.node, state: imageState(n.state), message: n.message || '', bad: n.state === 'Failed' })),
      failed: (i.nodes || []).filter((n) => n.state === 'Failed').map((n) => `${n.node}: ${n.message || 'failed'}`),
    }
  })
}

/// The pre-pull, which comes before any node moves: under way, or stopped with nothing
/// changed. `null` once a run is past it, or for a stop that has nothing to do with it.
export function prePull(st) {
  if (!st || st.prePulled || st.rollbackRun) return null
  const nodes = st.nodes || []
  const on = [...new Set(nodes.map((n) => n.release).filter(Boolean))]
  const stillOn = st.previousRelease || st.currentRelease || on.join(', ')
  if (st.phase === 'Failed' && nodes.some((n) => /^pre-pull/.test(n.message || ''))) {
    return { state: 'stopped', stillOn, mixed: on.length > 1, failed: nodes.filter((n) => n.phase === 'Failed').map((n) => n.name) }
  }
  if (nodes.some((n) => n.phase === 'PrePulling')) return { state: 'pulling', stillOn, mixed: on.length > 1, failed: [] }
  return null
}

/// A change between releases, in a line.
export function changeLine(c) {
  const short = (s) => (s && s.length > 16 ? s.slice(0, 12) : s)
  // The manifest's word for a component's image; the page says image (stormupdate#5).
  const kind = c.kind === 'golden' ? 'image' : c.kind
  const what = kind ? `${c.component} (${kind})` : c.component
  if (c.from && c.to) return `${what} ${short(c.from)} → ${short(c.to)}`
  return c.to ? `${what} added` : `${what} removed`
}
