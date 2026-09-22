<script>
  // The console's resource table.
  //
  // stormview's ComponentGrid is the generic renderer every storm app
  // shares; this is the console's own, because a console needs things a
  // shared grid should not assume: a status column that says "Ready"
  // rather than the feed's `ok`, a Kind column that disappears when every
  // row is the same kind, a header that stays put while you scroll a
  // hundred pods, and destructive actions kept to the right where they
  // are hard to hit by accident.
  //
  // Rows are feed components, and a relation is one of two things.
  //
  // `has_many` is containment — a pod's containers, a node's pods, a
  // golden's local copies — and nests into a table inside the row.
  // Everything else (`has_one`, `belongs_to`) is *context*: the node a VM
  // runs on, the volume a clone is stored in, the parent a snapshot was
  // cut from. Context is a reference, never containment and never where
  // clicking the row leads — reading it as a destination is how opening a
  // virtual machine landed in node details (#18).
  //
  // Clicking a line gives you that object. Where it has a page of its own
  // the row goes there; where it does not, the row opens in place, and
  // what opens is the whole of it: the detail unabbreviated, every metric,
  // the references as links, and every action including the ones kept off
  // the row.
  import StatusPill from './StatusPill.svelte'
  import EventBox from './EventBox.svelte'
  import { noteActivity } from '../stores.svelte.js'
  import Icon from './Icon.svelte'
  import ResourceTable from './ResourceTable.svelte'

  let {
    components = [],
    rootIds = [],
    invoke = null,
    showKind = true,
    level = 0,
    ancestors = new Set(),
  } = $props()

  const RANK = { error: 0, warn: 1, unknown: 2, idle: 3, ok: 4 }

  let sortKey = $state('label')
  let sortDir = $state(1)
  let expanded = $state({})
  let selected = $state([])
  let busy = $state(false)

  const byId = $derived(new Map(components.map((c) => [c.id, c])))
  const resolve = (id) => byId.get(id)

  const rows = $derived(rootIds.map(resolve).filter(Boolean))

  // Kind earns its column only when the rows actually differ.
  const kinds = $derived(new Set(rows.map((r) => r.kind)))
  const withKind = $derived(showKind && kinds.size > 1)

  // Where a row *is*, whatever that means for this kind of thing.
  //
  // Any `belongs_to` edge is a placement: a pod is in a namespace and on a
  // node, a drive is in a shelf, a volume is on an array, a local image is
  // on a node. Each one used to need its own hardcoded column here — there
  // were two, namespace and node, and every other placement in the feed
  // was invisible. They are read off the edge instead, which also gives
  // the column to the `FeedPlugin` upstreams whose components this repo
  // does not write.
  //
  // The name is the fact, not the id: a component id is not a path and
  // splitting it on ':' or '/' is a guess that goes wrong the first time a
  // name contains one. The feed's own label wins where the target is in
  // it.
  function placement(row, name) {
    const rel = (row.relations || []).find(
      (r) => r.kind === 'belongs_to' && r.name === name
    )
    // A placement is singular by definition: a pod is in one namespace and
    // on one node. An edge with several targets is a set of references —
    // the policies selecting a workload — and a column showing the first
    // of them would read as the only one.
    if (rel?.targets?.length !== 1) return ''
    // Only what is actually in the feed. A plugin publishes an edge
    // without being able to know whether the other side exists here — a
    // VM points at its Cilium endpoint on a console that may have no
    // Cilium — and falling back to the id's tail would invent a column
    // full of values naming objects that are not there.
    const target = byId.get(rel.targets[0])
    return target ? target.label : ''
  }

  // A placement earns a column when it tells the rows apart.
  //
  // "engine" is on every stormblock volume and is the same engine every
  // time; a column of one repeated value is a column of noise. Namespace
  // and node lead because that is the order people read them in, and the
  // rest follow in the order the feed declares them. Capped, because a
  // table with nine placement columns is as unreadable as one with none.
  const PLACEMENT_FIRST = ['namespace', 'node']
  const MAX_PLACEMENTS = 4

  const placements = $derived.by(() => {
    const names = []
    for (const r of rows) {
      for (const rel of r.relations || []) {
        if (rel.kind === 'belongs_to' && !names.includes(rel.name)) names.push(rel.name)
      }
    }
    return names
      .filter((n) => {
        const seen = new Set(rows.map((r) => placement(r, n)))
        seen.delete('')
        // Differing values, or present on some rows and not others — both
        // are facts about this list. One value on every row is not.
        return seen.size > 1 || (seen.size === 1 && rows.some((r) => !placement(r, n)))
      })
      .sort((a, b) => {
        const ia = PLACEMENT_FIRST.indexOf(a)
        const ib = PLACEMENT_FIRST.indexOf(b)
        return (ia < 0 ? 99 : ia) - (ib < 0 ? 99 : ib)
      })
      .slice(0, MAX_PLACEMENTS)
  })

  // References: every relation that is not containment, resolved to
  // somewhere to go. `href` on the relation wins (the feed saying where
  // this edge leads), then the target's own page, and failing both the
  // target as the root of a grid — so a reference is never a dead chip.
  function refs(row) {
    return (row.relations || [])
      .filter((r) => r.kind !== 'has_many')
      .flatMap((r) =>
        // Every target, not only the first. A reference relationship is
        // usually to one thing — the node a VM is on — but not always:
        // the policies that select a pod are several, and showing the
        // first of them is worse than showing none, because nothing says
        // there were others.
        (r.targets || []).map((id) => ({ rel: r, id }))
      )
      .map(({ rel, id }) => {
        const target = byId.get(id)
        // A plugin publishes an edge without being able to know whether
        // the other side is in the feed — a VM points at its Cilium
        // endpoint, and this console may have no Cilium. The renderer is
        // the only place that can tell, so it is where the dead ones go.
        if (!target && !rel.href) return null
        return {
          name: rel.name,
          label: target?.label || id.split(':').pop(),
          health: target?.health,
          href: rel.href || target?.link || `#/grid?id=${encodeURIComponent(id)}`,
        }
      })
      .filter(Boolean)
  }

  const sorted = $derived.by(() => {
    const k = sortKey
    const dir = sortDir
    return [...rows].sort((a, b) => {
      if (k === 'health') return ((RANK[a.health] ?? 9) - (RANK[b.health] ?? 9)) * dir
      if (k.startsWith('@')) {
        return placement(a, k.slice(1)).localeCompare(placement(b, k.slice(1))) * dir
      }
      return String(a[k] ?? '').localeCompare(String(b[k] ?? '')) * dir
    })
  })

  function sortBy(key) {
    if (sortKey === key) sortDir = -sortDir
    else {
      sortKey = key
      sortDir = 1
    }
  }

  function children(row) {
    return (row.relations || [])
      .filter((r) => r.kind === 'has_many')
      .map((r) => ({
        name: r.name,
        ids: r.targets.filter((t) => !ancestors.has(t) && byId.has(t)),
      }))
      .filter((s) => s.ids.length)
  }

  function toggleSelect(id) {
    selected = selected.includes(id) ? selected.filter((s) => s !== id) : [...selected, id]
  }

  function toggleAll() {
    selected = selected.length === rows.length ? [] : rows.map((r) => r.id)
  }

  // What just happened, so an action is not silent.
  //
  // `rowAction` caught its error and wrote it to console.error — so clicking
  // "Make golden" looked identical whether it started a download, was refused,
  // or never reached the server. An operator clicked, nothing moved, and there
  // was nothing on the page to say why.
  let notice = $state(null)
  let noticeTimer = null
  // Bumped after an action so an opened row re-reads its events: "did my
  // stop work" is asked immediately and answered a moment later.
  let eventTick = $state(0)

  function say(kind, text) {
    notice = { kind, text }
    clearTimeout(noticeTimer)
    // Errors stay until dismissed: a failure that vanishes while somebody is
    // reading it is the same as no message.
    if (kind === 'ok') noticeTimer = setTimeout(() => (notice = null), 6000)
  }

  async function run(action) {
    if (invoke) return invoke(action)
    // A route, not a request -- see `call()` in api.js.
    if (typeof action.path === 'string' && action.path.startsWith('#/')) {
      window.location.hash = action.path.slice(1)
      return
    }
    const resp = await fetch(action.path, { method: action.method || 'POST' })
    const data = await resp.json().catch(() => ({}))
    // A bare fetch resolves for a 500 as happily as for a 200, so the status
    // has to be checked or a refusal reads as success.
    if (!resp.ok) throw new Error(data.error || data.message || `${resp.status} ${resp.statusText}`)
    return data
  }

  async function rowAction(row, action) {
    if (action.danger && !confirm(`${action.label} ${row.label}?`)) return
    try {
      const data = await run(action)
      const msg = (data && (data.message || data.detail)) || `${action.label}: ${row.label}`
      say('ok', msg)
      eventTick++
      // Into the dock as well as onto the page. The notice here vanishes
      // when somebody navigates away, and "did that work" is asked for
      // rather longer than they stay on one list.
      noteActivity({ reason: action.label, message: msg, source: row.label })
    } catch (e) {
      say('err', `${action.label} ${row.label}: ${e.message}`)
      noteActivity({
        reason: action.label,
        message: e.message,
        source: row.label,
        warning: true,
      })
    }
  }

  // Bulk: whichever lifecycle actions every selected row offers enabled.
  const bulk = $derived.by(() => {
    const picked = selected.map(resolve).filter(Boolean)
    if (picked.length < 2) return []
    return ['start', 'stop', 'restart']
      .map((id) => {
        const acts = picked
          .map((r) => (r.actions || []).find((a) => a.id === id && a.enabled))
          .filter(Boolean)
        return acts.length === picked.length ? { id, label: acts[0].label, acts } : null
      })
      .filter(Boolean)
  })

  async function runBulk(b) {
    if (!confirm(`${b.label} ${b.acts.length} components?`)) return
    busy = true
    for (const a of b.acts) {
      try {
        await run(a)
      } catch (e) {
        console.error(e)
      }
    }
    busy = false
    selected = []
  }

  // Clicking a line gives you that object: its own page where it has one,
  // and where it has none the row opens in place rather than doing
  // nothing — which is what a pod, a container and a volume did.
  const open = (row) => {
    if (row.link) location.hash = row.link
    else expanded[row.id] = !expanded[row.id]
  }
  const arrow = (key) => (sortKey !== key ? '' : sortDir > 0 ? '▲' : '▼')

  // A relation name is written for a machine (`has_many`, `local_copies`);
  // a column heading is read by a person.
  const title = (n) => n.replace(/_/g, ' ').replace(/^./, (c) => c.toUpperCase())

  // The expanded row spans the table. This was a literal that counted
  // seven columns and knew about Kind, so every placement column it did
  // not know about left the nested content short of the right edge.
  const cols = $derived(
    2 + 1 + placements.length + 1 + (withKind ? 1 : 0) + 1 + 1 + 1
  )

  // A drive carries nine operations and a VM half a dozen. Nine buttons
  // on every row is a wall, not a set of choices — and it puts a
  // destructive one a mis-click away from a harmless one. So the two
  // safe, most-used actions stay on the row and everything else, every
  // destructive action included, goes behind the row menu. The same rule
  // OpenShift's kebab and ESXi's Actions menu follow.
  const INLINE = 2

  // Which two get the line, in order of what somebody came to the row to do.
  //
  // Taking the first two in declaration order buried a VM's Console behind
  // the kebab under restart and stop -- the one action people open the list
  // *for*, two clicks away, while the ones they use rarely sat in the open.
  // Getting to a screen is the reason a virtual machine list exists.
  //
  // Anything not named keeps declaration order after the named ones, so a
  // plugin that grows a new action does not have to be known about here.
  const FIRST = ['console', 'start']

  function split(row) {
    const acts = row.actions || []
    const safe = acts.filter((a) => !a.danger)
    const ranked = [
      ...FIRST.map((id) => safe.find((a) => a.id === id)).filter(Boolean),
      ...safe.filter((a) => !FIRST.includes(a.id)),
    ]
    const inline = ranked.slice(0, INLINE)
    return { inline, menu: acts.filter((a) => !inline.includes(a)) }
  }

  let menuFor = $state(null)

  function toggleMenu(e, id) {
    e.stopPropagation()
    menuFor = menuFor === id ? null : id
  }

  $effect(() => {
    if (!menuFor) return
    const close = () => (menuFor = null)
    window.addEventListener('click', close, { once: true })
    return () => window.removeEventListener('click', close)
  })

  // Right-click on an action that has alternatives.
  //
  // A VM's Console button opens the screen, which is what "console" means
  // when a machine has one. A serial line is the specialist answer — wanted
  // exactly when the screen is blank — so it is on the menu rather than
  // taking a second button in every row.
  //
  // The alternatives travel in the route as `alt=`, because a menu on one
  // kind of row is not worth a new field in the action model and in every
  // renderer that reads it.
  let menu = $state(null)

  function altsOf(action) {
    if (typeof action?.path !== 'string' || !action.path.includes('?')) return []
    const q = new URLSearchParams(action.path.split('?')[1])
    const alt = q.get('alt')
    if (!alt) return []
    const here = q.get('door')
    const label = (d) => (d === 'serial' ? 'Serial console' : 'Screen')
    const base = action.path.split('?')[0]
    // Each door twice: as it opens by default, and full screen.
    //
    // Full screen is a property of how you want to look at a console, not of
    // which console it is, so it belongs beside each door rather than as a
    // mode to find once you are already inside one.
    const doors = [here, ...alt.split(',')].filter(Boolean)
    return doors.flatMap((d) => [
      { label: label(d), path: `${base}?door=${d}` },
      { label: `${label(d)}, full screen`, path: `${base}?door=${d}&full=1` },
    ])
  }

  function openMenu(e, action) {
    const alts = altsOf(action)
    if (!alts.length) return
    e.preventDefault()
    menu = { x: e.clientX, y: e.clientY, items: alts }
  }

  function pick(item) {
    menu = null
    window.location.hash = item.path.slice(1)
  }

</script>

{#if notice}
  <div class="sc-notice" class:err={notice.kind === 'err'} role="status">
    <span>{notice.text}</span>
    <button class="dismiss" onclick={() => (notice = null)} aria-label="Dismiss">×</button>
  </div>
{/if}

{#if bulk.length}
  <div class="bulk">
    <span><strong>{selected.length}</strong> selected</span>
    {#each bulk as b}
      <button disabled={busy} onclick={() => runBulk(b)}>{b.label} all</button>
    {/each}
    <button class="clear" onclick={() => (selected = [])}>Clear selection</button>
  </div>
{/if}

<div class="wrap" class:nested={level > 0}>
  <table>
    <thead>
      <tr>
        <th class="ctl"></th>
        <th class="ctl">
          <input
            type="checkbox"
            aria-label="Select all"
            checked={rows.length > 0 && selected.length === rows.length}
            onchange={toggleAll}
          />
        </th>
        <th class="sortable name" onclick={() => sortBy('label')}>Name <i>{arrow('label')}</i></th>
        {#each placements as pname (pname)}
          <th class="sortable place" onclick={() => sortBy(`@${pname}`)}
            >{title(pname)} <i>{arrow(`@${pname}`)}</i></th
          >
        {/each}
        <th class="sortable status" onclick={() => sortBy('health')}>Status <i>{arrow('health')}</i></th>
        {#if withKind}
          <th class="sortable kind" onclick={() => sortBy('kind')}>Kind <i>{arrow('kind')}</i></th>
        {/if}
        <th class="sortable" onclick={() => sortBy('detail')}>Detail <i>{arrow('detail')}</i></th>
        <th>Metrics</th>
        <th class="acts"></th>
      </tr>
    </thead>
    <tbody>
      {#each sorted as row (row.id)}
        {@const kids = children(row)}
        {@const acts = split(row)}
        <tr
          class:selected={selected.includes(row.id)}
          class:clickable={!!row.link}
          onclick={() => open(row)}
        >
          <td class="ctl">
            <button
              class="expander"
              aria-label={expanded[row.id] ? 'Collapse' : 'Expand'}
              aria-expanded={!!expanded[row.id]}
              onclick={(e) => { e.stopPropagation(); expanded[row.id] = !expanded[row.id] }}
            >
              <span class="caret" class:down={expanded[row.id]}><Icon name="chevron" size={12} stroke={2.2} /></span>
            </button>
          </td>
          <td class="ctl" onclick={(e) => e.stopPropagation()}>
            <input
              type="checkbox"
              aria-label="Select {row.label}"
              checked={selected.includes(row.id)}
              onchange={() => toggleSelect(row.id)}
            />
          </td>
          <td class="name">{row.label}</td>
          {#each placements as pname (pname)}
            <td class="place">{placement(row, pname)}</td>
          {/each}
          <td class="status"><StatusPill health={row.health} /></td>
          {#if withKind}<td class="kind">{row.kind}</td>{/if}
          <td class="detail">{row.detail ?? ''}</td>
          <td class="metrics">
            {#each row.metrics || [] as m}
              <span class="m">
                <span class="ml">{m.label}</span>
                <span class="mv {m.tone || ''}">{m.value}{m.unit || ''}</span>
              </span>
            {/each}
          </td>
          <td class="acts" onclick={(e) => e.stopPropagation()}>
            {#each acts.inline as a}
              <button
                class:ok={a.id === 'start'}
                class:warn={a.id === 'restart'}
                disabled={!a.enabled}
                onclick={() => rowAction(row, a)}>{a.label}</button
              >
            {/each}
            {#if acts.menu.length}
              <span class="menu-wrap">
                <button
                  class="kebab"
                  aria-label="More actions for {row.label}"
                  aria-expanded={menuFor === row.id}
                  onclick={(e) => toggleMenu(e, row.id)}>⋯</button
                >
                {#if menuFor === row.id}
                  <div class="menu" role="menu">
                    {#each acts.menu as a}
                      <button
                        role="menuitem"
                        class:danger={a.danger}
                        disabled={!a.enabled}
                        onclick={() => { menuFor = null; rowAction(row, a) }}>{a.label}</button
                      >
                    {/each}
                  </div>
                {/if}
              </span>
            {/if}
          </td>
        </tr>
        {#if expanded[row.id]}
          {@const rs = refs(row)}
          <tr class="child">
            <td colspan={cols}>
              <div class="open">
                {#if row.detail}
                  <p class="full-detail">{row.detail}</p>
                {/if}

                {#if row.metrics?.length}
                  <dl class="facts">
                    {#each row.metrics as m}
                      <div><dt>{m.label}</dt><dd class="{m.tone || ''}">{m.value}{m.unit || ''}</dd></div>
                    {/each}
                  </dl>
                {/if}

                {#if rs.length}
                  <div class="section">
                    <div class="section-title">references</div>
                    <div class="chips">
                      {#each rs as r}
                        <a class="chip" href={r.href}>
                          <span class="cn">{title(r.name)}</span>
                          <span class="cv">{r.label}</span>
                        </a>
                      {/each}
                    </div>
                  </div>
                {/if}

                {#if (row.actions || []).length}
                  <div class="section">
                    <div class="section-title">actions</div>
                    <div class="all-acts">
                      {#each row.actions as a}
                        <button
                          class:ok={a.id === 'start'}
                          class:warn={a.id === 'restart'}
                          class:danger={a.danger}
                          disabled={!a.enabled}
                          title={altsOf(a).length ? 'Right-click for more' : null}
                          oncontextmenu={(e) => openMenu(e, a)}
                          onclick={() => rowAction(row, a)}>{a.label}</button
                        >
                      {/each}
                    </div>
                  </div>
                {/if}

                <!-- What happened to it, wherever a row opens. The
                     object's own fields say what it is now; only this says
                     how it got there. -->
                <div class="section">
                  <EventBox id={row.id} compact={true} refresh={eventTick} />
                </div>

                {#if !row.detail && !row.metrics?.length && !rs.length && !(row.actions || []).length && !kids.length}
                  <p class="full-detail dim">Nothing more than the row: no detail, metrics, references or actions.</p>
                {/if}

                {#each kids as s (s.name)}
                  <div class="section">
                    <div class="section-title">{title(s.name)} <span>{s.ids.length}</span></div>
                    <ResourceTable
                      {components}
                      rootIds={s.ids}
                      {invoke}
                      {showKind}
                      level={level + 1}
                      ancestors={new Set([...ancestors, row.id])}
                    />
                  </div>
                {/each}
              </div>
            </td>
          </tr>
        {/if}
      {/each}
    </tbody>
  </table>
</div>

{#if menu}
  <!-- Click anywhere else to dismiss, which is what a menu does. -->
  <div
    class="menu-scrim"
    role="presentation"
    onclick={() => (menu = null)}
    oncontextmenu={(e) => { e.preventDefault(); menu = null }}
  ></div>
  <ul class="ctxmenu" style="left:{menu.x}px; top:{menu.y}px" role="menu">
    {#each menu.items as item}
      <li role="none">
        <button role="menuitem" onclick={() => pick(item)}>{item.label}</button>
      </li>
    {/each}
  </ul>
{/if}

<style>
  .menu-scrim { position: fixed; inset: 0; z-index: 70; }
  .ctxmenu {
    position: fixed;
    z-index: 71;
    margin: 0;
    padding: 4px;
    list-style: none;
    min-width: 168px;
    background: var(--panel-raised);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    box-shadow: 0 6px 24px rgb(0 0 0 / 28%);
  }
  .ctxmenu button {
    display: block;
    width: 100%;
    text-align: left;
    padding: 6px 10px;
    border: none;
    background: none;
    color: var(--text);
    font-size: var(--sc-t-meta);
    border-radius: var(--radius-sm);
    cursor: pointer;
  }
  .ctxmenu button:hover { background: var(--nav-hover); }

  .bulk {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 8px 12px;
    margin-bottom: 10px;
    background: var(--accent-bg);
    border: 1px solid var(--border-strong);
    border-radius: var(--radius);
    font-size: var(--sc-t-body);
  }
  .bulk .clear { margin-left: auto; }
  .bulk button { font-size: var(--sc-t-meta); padding: 4px 10px; }

  .wrap {
    background: var(--panel);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    overflow: auto;
    /* Bounded so the header can stay put over a long list. */
    max-height: calc(100vh - var(--nav-h) - 200px);
  }
  .wrap.nested {
    background: var(--panel-raised);
    max-height: none;
    overflow: visible;
  }

  table { width: 100%; border-collapse: collapse; }

  th {
    text-align: left;
    padding: var(--sc-row-py) var(--sc-row-px);
    font-size: var(--sc-t-meta);
    font-weight: 600;
    color: var(--text-dim);
    background: color-mix(in srgb, var(--panel-raised) 55%, var(--panel));
    border-bottom: 1px solid var(--border);
    white-space: nowrap;
    user-select: none;
    position: sticky;
    top: 0;
    z-index: 1;
  }
  .nested th { position: static; }
  th.sortable { cursor: pointer; }
  th.sortable:hover { color: var(--text); }
  th i { font-style: normal; font-size: 9px; color: var(--accent); }

  td {
    padding: var(--sc-row-py) var(--sc-row-px);
    font-size: var(--sc-t-body);
    border-bottom: 1px solid var(--sc-hairline);
    color: var(--text);
    vertical-align: middle;
  }
  tbody tr:last-child > td { border-bottom: none; }
  tr.clickable { cursor: pointer; }
  /* Zebra striping is a Clarity signature; the openshift style sets the
     token to transparent and gets hairline-separated rows instead. */
  tbody tr:nth-child(even):not(.child) { background: var(--sc-zebra); }
  tbody tr:hover:not(.child) { background: var(--nav-hover); }
  tr.selected { background: var(--accent-bg); }

  th.ctl, td.ctl { width: 30px; padding-left: 10px; padding-right: 0; }
  input[type='checkbox'] { accent-color: var(--accent); }

  .expander {
    background: none;
    border: none;
    color: var(--text-faint);
    padding: 2px;
    display: grid;
    place-items: center;
  }
  .expander:hover { color: var(--text); background: none; }
  .caret { display: grid; transition: transform 0.15s ease; }
  .caret.down { transform: rotate(90deg); }

  .name { font-weight: 500; }
  .status { width: 110px; }
  .kind { color: var(--text-dim); font-size: var(--sc-t-meta); white-space: nowrap; }
  .sc-notice {
    display: flex; align-items: center; gap: 10px; justify-content: space-between;
    padding: 8px 12px; margin-bottom: 10px; border-radius: var(--radius-sm);
    background: var(--ok-bg); color: var(--ok); border: 1px solid var(--ok-border);
    font-size: var(--sc-t-meta);
  }
  .sc-notice.err { background: var(--error-bg); color: var(--error); border-color: var(--error-border); }
  .sc-notice .dismiss {
    background: none; border: 0; color: inherit; cursor: pointer;
    font-size: 16px; line-height: 1; padding: 0 2px;
  }
  .place { white-space: nowrap; color: var(--text-dim); }
  .detail { color: var(--text-dim); }

  .metrics { white-space: nowrap; }
  .m { display: inline-flex; gap: 5px; align-items: baseline; margin-right: 14px; }
  .ml {
    font-size: var(--sc-t-eyebrow);
    text-transform: uppercase;
    letter-spacing: 0.05em;
    color: var(--text-faint);
  }
  .mv { font-family: var(--mono); font-size: var(--sc-t-meta); font-weight: 600; }
  .mv.ok { color: var(--ok); }
  .mv.warn { color: var(--warn-strong); }
  .mv.error { color: var(--error); }
  .mv.muted { color: var(--text-dim); font-weight: 400; }
  .mv.accent { color: var(--accent); }

  .acts { text-align: right; white-space: nowrap; }
  .acts button { font-size: var(--sc-t-eyebrow); padding: 3px 9px; margin-left: 4px; }

  .menu-wrap { position: relative; display: inline-block; }
  .kebab {
    font-size: 15px;
    line-height: 1;
    padding: 2px 8px;
    letter-spacing: 1px;
  }
  .menu {
    position: absolute;
    right: 0;
    top: calc(100% + 4px);
    z-index: 30;
    min-width: 190px;
    display: grid;
    padding: 4px;
    background: var(--panel);
    border: 1px solid var(--border-strong);
    border-radius: var(--radius);
    box-shadow: 0 6px 20px rgb(0 0 0 / 0.28);
  }
  .menu button {
    margin: 0;
    width: 100%;
    text-align: left;
    background: none;
    border: none;
    border-radius: var(--radius-sm);
    padding: 6px 10px;
    font-size: var(--sc-t-body);
    color: var(--text);
  }
  .menu button:hover:not(:disabled) { background: var(--nav-hover); }
  .menu button:disabled { color: var(--text-ghost); }
  .menu button.danger { color: var(--error); }
  .menu button.danger:hover:not(:disabled) {
    background: color-mix(in srgb, var(--error) 16%, transparent);
  }

  .child > td { padding: 6px 14px 14px 40px; background: color-mix(in srgb, var(--panel-raised) 35%, transparent); }
  .open { display: grid; gap: 10px; }
  .full-detail { margin: 0; font-size: var(--sc-t-body); color: var(--text); }
  .full-detail.dim { color: var(--text-faint); }

  /* Every metric, laid out to be read rather than scanned past — the row
     shows them in one line and runs out of width long before the feed
     runs out of facts. */
  .facts { display: flex; flex-wrap: wrap; gap: 4px 22px; margin: 0; }
  .facts > div { display: grid; gap: 1px; }
  .facts dt {
    font-size: var(--sc-t-eyebrow);
    text-transform: uppercase;
    letter-spacing: 0.05em;
    color: var(--text-faint);
  }
  .facts dd {
    margin: 0;
    font-family: var(--mono);
    font-size: var(--sc-t-meta);
    font-weight: 600;
    color: var(--text);
  }
  .facts dd.ok { color: var(--ok); }
  .facts dd.warn { color: var(--warn-strong); }
  .facts dd.error { color: var(--error); }
  .facts dd.muted { color: var(--text-dim); font-weight: 400; }
  .facts dd.accent { color: var(--accent); }

  .chips { display: flex; flex-wrap: wrap; gap: 6px; }
  .chip {
    display: inline-flex;
    align-items: baseline;
    gap: 6px;
    padding: 3px 9px;
    border: 1px solid var(--border);
    border-radius: 999px;
    background: var(--panel);
    text-decoration: none;
    font-size: var(--sc-t-meta);
    color: var(--text);
  }
  .chip:hover { border-color: var(--accent); }
  .chip .cn {
    font-size: var(--sc-t-eyebrow);
    text-transform: uppercase;
    letter-spacing: 0.05em;
    color: var(--text-faint);
  }
  .chip .cv { font-family: var(--mono); }

  .all-acts { display: flex; flex-wrap: wrap; gap: 6px; }
  .all-acts button { font-size: var(--sc-t-meta); padding: 4px 11px; }
  .all-acts button.danger { color: var(--error); border-color: var(--error-border); }

  .section + .section { margin-top: 0; }
  .section-title {
    font-size: var(--sc-t-eyebrow);
    text-transform: uppercase;
    letter-spacing: 0.06em;
    color: var(--text-faint);
    margin-bottom: 5px;
  }
  .section-title span { color: var(--text-ghost); }
</style>
