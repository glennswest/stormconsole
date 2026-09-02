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
  // Rows are feed components. Relations that point downward expand into
  // nested tables; `belongs_to` stays upward and is not expanded.
  import StatusPill from './StatusPill.svelte'
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

  const sorted = $derived.by(() => {
    const k = sortKey
    const dir = sortDir
    return [...rows].sort((a, b) => {
      if (k === 'health') return ((RANK[a.health] ?? 9) - (RANK[b.health] ?? 9)) * dir
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
      .filter((r) => r.kind !== 'belongs_to')
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

  async function run(action) {
    if (invoke) return invoke(action)
    return fetch(action.path, { method: action.method || 'POST' })
  }

  async function rowAction(row, action) {
    if (action.danger && !confirm(`${action.label} ${row.label}?`)) return
    try {
      await run(action)
    } catch (e) {
      console.error(e)
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

  const open = (row) => { if (row.link) location.hash = row.link }
  const arrow = (key) => (sortKey !== key ? '' : sortDir > 0 ? '▲' : '▼')
</script>

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
        <tr
          class:selected={selected.includes(row.id)}
          class:clickable={!!row.link}
          onclick={() => open(row)}
        >
          <td class="ctl">
            {#if kids.length}
              <button
                class="expander"
                aria-label={expanded[row.id] ? 'Collapse' : 'Expand'}
                aria-expanded={!!expanded[row.id]}
                onclick={(e) => { e.stopPropagation(); expanded[row.id] = !expanded[row.id] }}
              >
                <span class="caret" class:down={expanded[row.id]}><Icon name="chevron" size={12} stroke={2.2} /></span>
              </button>
            {/if}
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
            {#each row.actions || [] as a}
              <button
                class:ok={a.id === 'start'}
                class:warn={a.id === 'restart'}
                class:danger={a.danger}
                disabled={!a.enabled}
                onclick={() => rowAction(row, a)}>{a.label}</button
              >
            {/each}
          </td>
        </tr>
        {#if expanded[row.id] && kids.length}
          <tr class="child">
            <td colspan={withKind ? 8 : 7}>
              {#each kids as s (s.name)}
                <div class="section">
                  <div class="section-title">{s.name.replace(/_/g, ' ')} <span>{s.ids.length}</span></div>
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
            </td>
          </tr>
        {/if}
      {/each}
    </tbody>
  </table>
</div>

<style>
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
    padding: 8px 14px;
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
    padding: 8px 14px;
    font-size: var(--sc-t-body);
    border-bottom: 1px solid var(--sc-hairline);
    color: var(--text);
    vertical-align: middle;
  }
  tbody tr:last-child > td { border-bottom: none; }
  tr.clickable { cursor: pointer; }
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

  .child > td { padding: 6px 14px 14px 40px; background: color-mix(in srgb, var(--panel-raised) 35%, transparent); }
  .section + .section { margin-top: 10px; }
  .section-title {
    font-size: var(--sc-t-eyebrow);
    text-transform: uppercase;
    letter-spacing: 0.06em;
    color: var(--text-faint);
    margin-bottom: 5px;
  }
  .section-title span { color: var(--text-ghost); }
</style>
