// A drive's actions, grouped (#308). Pure, so it runs under node.
//
// stormdrive's feed gives each drive a flat list (components.rs): locate,
// tests, fleet join/leave, enrol, overcommit, designations, acknowledge,
// forget. The menu groups them by what they are for, and puts every
// destructive one in a group of its own at the end, so a harmless click is
// never next to one that destroys data. Format is the console's own (the
// dialog), never the feed's: its `format-4k` formatted without PI.

const GROUPS = [
  ['Locate', (id) => id.startsWith('locate')],
  ['Test', (id) => id.startsWith('test')],
  ['Fleet', (id) => id.startsWith('fleet') || id === 'enroll' || id === 'drain' || id.startsWith('drain')],
  ['Designate', (id) => id.startsWith('mark') || id.startsWith('designat') || id.startsWith('overcommit')],
]

function groupOf(id) {
  return (GROUPS.find(([, f]) => f(id)) || ['Other'])[0]
}

/// `actions`: the feed's, each `{id, label, enabled, danger, why?, count?}`.
/// `opts.format`, `opts.replace`, `opts.firmware`: `{enabled, why}` to offer
/// Format…, Replace…, Update firmware…, or null.
/// → `[{name, danger, items: [{key, label, enabled, danger, why, action|format}]}]`.
export function groupActions(actions, opts = {}) {
  const groups = new Map()
  const put = (name, item) => {
    if (!groups.has(name)) groups.set(name, [])
    groups.get(name).push(item)
  }
  for (const a of actions || []) {
    if (a.id?.startsWith('format')) continue
    // A RAID controller's verbs (#325): Set good, Set JBOD, Switch to HBA. stormdrive marks them danger, and
    // the storage guard asks for the serial; they destroy nothing, so they are their own group, not Destructive.
    if (a.id?.startsWith('controller-') || a.id?.startsWith('personality-')) {
      put('Controller', { key: a.id, label: a.label, enabled: !!a.enabled, danger: false, why: a.why || '', count: a.count, action: a })
      continue
    }
    const item = { key: a.id, label: a.label, enabled: !!a.enabled, danger: !!a.danger, why: a.why || '', count: a.count, action: a }
    put(a.danger ? 'Destructive' : groupOf(a.id || ''), item)
  }
  // Replace and firmware (#317) are the console's own dialogs, like Format:
  // neither destroys data, so they sit before the destructive group.
  for (const [k, label] of [['replace', 'Replace…'], ['firmware', 'Update firmware…']]) {
    const o = opts[k]
    if (o) put('Maintain', { key: k, label, enabled: !!o.enabled, danger: false, why: o.why || '', [k]: true })
  }
  if (opts.format) {
    // Format is the first destructive item: it is what the owner came for.
    const list = groups.get('Destructive') || []
    list.unshift({ key: 'format', label: 'Format…', enabled: !!opts.format.enabled, danger: true, why: opts.format.why || '', format: true })
    groups.set('Destructive', list)
  }
  const order = ['Controller', 'Locate', 'Test', 'Fleet', 'Designate', 'Other', 'Maintain', 'Destructive']
  return order.filter((n) => groups.has(n)).map((name) => ({ name, danger: name === 'Destructive', items: groups.get(name) }))
}

/// The actions every drive in a selection offers, by id: enabled when it is
/// enabled on at least one, with `count` saying on how many. Running one runs
/// it on each drive where it is enabled.
export function commonActions(drives) {
  if (!drives.length) return []
  const byId = new Map()
  for (const d of drives) {
    for (const a of d.actions || []) {
      if (!byId.has(a.id)) byId.set(a.id, { ...a, count: 0, on: [] })
      const e = byId.get(a.id)
      if (a.enabled) {
        e.count += 1
        e.on.push({ drive: d, action: a })
      }
    }
  }
  return [...byId.values()].map((e) => ({ ...e, enabled: e.count > 0 }))
}
