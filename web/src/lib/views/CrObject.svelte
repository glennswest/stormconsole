<script>
  // One custom resource, all of it (#348): its printer columns first (what
  // the grid row showed), spec and status as sections, conditions as a
  // table, owners as links, its events, and the YAML with copy. Edit and
  // Delete are done as you, and a refusal is the apiserver's own words.
  import { route } from '../router.svelte.js'
  import { get, call } from '../api.js'
  import { ago } from '../ui/time.js'
  import { cellText, conditionTone, fieldRows, curatedCell } from '../crgrid.js'
  import PageHeader from '../components/PageHeader.svelte'
  import EmptyState from '../components/EmptyState.svelte'
  import YamlPanel from '../components/YamlPanel.svelte'
  import CopyButton from '../components/CopyButton.svelte'

  const TABS = ['Details', 'YAML', 'Events']
  const crd = route.current.params.crd
  const ns = route.current.params.ns === '_' ? '' : route.current.params.ns
  const name = route.current.params.name
  const path = `/api/plugins/k8s/cr/${crd}/object/${ns || '_'}/${encodeURIComponent(name)}`

  let tab = $state(TABS.includes(route.current.query.get('tab')) ? route.current.query.get('tab') : 'Details')
  let d = $state(null)
  let error = $state('')
  let note = $state('')

  async function load() {
    try {
      d = await get(path)
      error = ''
    } catch (e) {
      error = e.message
    }
  }
  load()
  // An object changes under you; the page follows it while it is open.
  const timer = setInterval(() => tab !== 'YAML' && load(), 5000)
  $effect(() => () => clearInterval(timer))

  async function remove() {
    const typed = window.prompt(`Delete ${d.kind.kind} ${name}${ns ? ` in ${ns}` : ''}? It is done as you.\n\nType ${name} to confirm.`)
    if (typed === null) return
    if (typed.trim() !== name) {
      note = `Not deleted: that is not ${name}.`
      return
    }
    try {
      await call('DELETE', path)
      location.hash = `#/cr/${crd}${ns ? `?ns=${encodeURIComponent(ns)}` : ''}`
    } catch (e) {
      if (e.message !== 'cancelled') note = `Delete refused: ${e.message}`
    }
  }

  const listHref = `#/cr/${crd}${ns ? `?ns=${encodeURIComponent(ns)}` : ''}`
  const spec = $derived(fieldRows(d?.object?.spec))
  const status = $derived(fieldRows(Object.fromEntries(Object.entries(d?.object?.status || {}).filter(([k]) => k !== 'conditions'))))
  const labels = $derived(Object.entries(d?.object?.metadata?.labels || {}))
  // A printer column's path, so a field it prints is marked in the sections.
  const printed = $derived(new Set((d?.columns || []).map((c) => c.path.replace(/^\./, ''))))
</script>

<div class="sc-page">
  <PageHeader
    crumbs={[
      { label: 'Custom resources', href: '#/crds' },
      { label: d?.kind?.kind || crd, href: listHref },
      ...(ns ? [{ label: ns, href: `#/k8s/ns/${encodeURIComponent(ns)}` }] : []),
      { label: name },
    ]}
    title={name}
    scope={d ? d.kind.kind : ''}
  >
    {#snippet actions()}
      {#if d}
        <button onclick={() => (tab = 'YAML')} disabled={!d.editable} title={d.editable ? 'Edit the YAML, saved as you' : 'Your rights do not include update'}>Edit</button>
        <button class="danger" onclick={remove} disabled={!d.deletable} title={d.deletable ? 'Delete, as you' : 'Your rights do not include delete'}>Delete</button>
      {/if}
    {/snippet}
  </PageHeader>

  {#if error && !d}
    <EmptyState icon="cluster" title="This object cannot be read" hint={error}>
      {#snippet action()}
        <a class="sc-back" href={listHref}>All of this kind</a>
      {/snippet}
    </EmptyState>
  {:else if !d}
    <div class="sc-empty"><p>Reading {name}…</p></div>
  {:else}
    {#if note}<p class="note">{note}</p>{/if}
    {#if error}<p class="note">Showing the last answer — {error}</p>{/if}
    <nav class="tabs" aria-label="Object sections">
      {#each TABS as t}
        <button class:active={tab === t} aria-current={tab === t ? 'page' : undefined} onclick={() => (tab = t)}>
          {t}{#if t === 'Events' && d.events?.length} ({d.events.length}){/if}
        </button>
      {/each}
    </nav>

    {#if tab === 'Details'}
      {#if d.columns.length}
        <section class="printed" aria-label="Printer columns">
          {#each d.columns as c}
            {@const cur = curatedCell(crd, { ...c, printer: true }, c.value)}
            <div class="pc" title={`${c.path}${c.description ? ` — ${c.description}` : ''}`}>
              <span class="k">{c.name}{#if c.priority > 0} <span class="dim">(wide)</span>{/if}</span>
              <span class="v mono {cur?.tone || ''}">{cur ? cur.value : cellText(c, c.value) || '—'}</span>
            </div>
          {/each}
        </section>
      {/if}

      <section class="cards">
        <div class="card">
          <h2>Object</h2>
          <dl>
            <dt>Name</dt><dd class="mono">{name} <CopyButton value={name} /></dd>
            {#if ns}<dt>Project</dt><dd><a href={`#/k8s/ns/${encodeURIComponent(ns)}`}>{ns}</a></dd>{/if}
            <dt>Kind</dt><dd class="mono">{d.object.apiVersion} {d.object.kind}</dd>
            <dt>Created</dt><dd title={d.object.metadata?.creationTimestamp}>{ago(d.object.metadata?.creationTimestamp)} ago</dd>
            <dt>Owners</dt>
            <dd>
              {#each d.owners as o}
                <div>
                  {#if o.href}<a href={o.href}>{o.kind} {o.name}</a>{:else}<span class="mono">{o.kind} {o.name}</span>{/if}
                  {#if o.controller}<span class="dim"> (controller)</span>{/if}
                </div>
              {:else}
                <span class="dim">none</span>
              {/each}
            </dd>
            {#if labels.length}
              <dt>Labels</dt>
              <dd>{#each labels as [k, v]}<span class="label mono">{k}={v}</span>{/each}</dd>
            {/if}
          </dl>
        </div>
      </section>

      {#if d.conditions.length}
        <div class="card wide">
          <h2>Conditions</h2>
          <table>
            <thead><tr><th>Type</th><th>Status</th><th>Reason</th><th>Message</th><th>Last transition</th></tr></thead>
            <tbody>
              {#each d.conditions as c}
                <tr>
                  <td class="mono">{c.type}</td>
                  <td class={conditionTone(c)}>{c.status}</td>
                  <td>{c.reason}</td>
                  <td>{c.message}</td>
                  <td title={c.lastTransitionTime}>{c.lastTransitionTime ? `${ago(c.lastTransitionTime)} ago` : ''}</td>
                </tr>
              {/each}
            </tbody>
          </table>
        </div>
      {/if}

      {#each [['Spec', spec, 'spec'], ['Status', status, 'status']] as [label, fields, root]}
        {#if fields.length}
          <div class="card wide">
            <h2>{label}</h2>
            <dl>
              {#each fields as f}
                <dt class:hl={printed.has(`${root}.${f.key}`)} title={printed.has(`${root}.${f.key}`) ? 'A printer column' : ''}>{f.key}</dt>
                <dd>{#if f.nested}<pre>{f.nested}</pre>{:else}<span class="mono">{f.text}</span>{/if}</dd>
              {/each}
            </dl>
          </div>
        {/if}
      {/each}
    {:else if tab === 'YAML'}
      <div class="yamlbar"><CopyButton value={d.yaml} label="Copy the YAML of" /></div>
      <YamlPanel yaml={d.yaml} savePath={d.editable ? path : ''} label={`${d.kind.kind} ${name}`} />
    {:else if tab === 'Events'}
      {#if d.events === null}
        <p class="dim">Events cannot be read as you.</p>
      {:else if !d.events.length}
        <p class="dim">Nothing has been recorded about {name}.</p>
      {:else}
        <div class="card wide">
          <table>
            <thead><tr><th>When</th><th>Type</th><th>Reason</th><th>Message</th><th>Count</th></tr></thead>
            <tbody>
              {#each d.events as e}
                <tr>
                  <td title={e.time}>{ago(e.time)}</td>
                  <td class={e.type === 'Warning' ? 'warn' : ''}>{e.type}</td>
                  <td>{e.reason}</td>
                  <td>{e.message}</td>
                  <td>{e.count}</td>
                </tr>
              {/each}
            </tbody>
          </table>
        </div>
      {/if}
    {/if}
  {/if}
</div>

<style>
  .tabs { display: flex; gap: 2px; border-bottom: 1px solid var(--border); margin-bottom: 14px; }
  .tabs button {
    background: none; border: none; border-bottom: 2px solid transparent; border-radius: 0;
    padding: 7px 14px; font-size: var(--sc-t-body); color: var(--text-dim);
  }
  .tabs button:hover { color: var(--text); background: var(--nav-hover); }
  .tabs button.active { color: var(--text); border-bottom-color: var(--accent); font-weight: 600; }
  .printed { display: flex; flex-wrap: wrap; gap: 8px; margin-bottom: 12px; }
  .pc {
    display: flex; flex-direction: column; gap: 2px; padding: 8px 12px; min-width: 110px;
    background: var(--panel); border: 1px solid var(--border); border-top: 2px solid var(--accent); border-radius: var(--radius-sm);
  }
  .pc .k { font-size: var(--sc-t-eyebrow); text-transform: uppercase; letter-spacing: 0.05em; color: var(--text-faint); }
  .pc .v { font-size: var(--sc-t-body); font-weight: 600; }
  .cards { display: grid; grid-template-columns: repeat(auto-fit, minmax(300px, 1fr)); gap: 12px; margin-bottom: 12px; }
  .card { background: var(--panel); border: 1px solid var(--border); border-radius: var(--radius); padding: 14px var(--sc-row-px, 14px); }
  .card.wide { margin-bottom: 12px; overflow-x: auto; }
  h2 { font-size: var(--sc-t-eyebrow); text-transform: uppercase; letter-spacing: 0.06em; color: var(--text-faint); margin: 0 0 10px; }
  dl { display: grid; grid-template-columns: max-content 1fr; gap: 6px 14px; margin: 0; }
  dt { color: var(--text-faint); font-size: var(--sc-t-meta); }
  dt.hl { color: var(--accent); font-weight: 600; }
  dd { margin: 0; min-width: 0; overflow-wrap: anywhere; }
  pre { margin: 0; font-size: 12px; white-space: pre-wrap; }
  table { width: 100%; border-collapse: collapse; }
  th { text-align: left; color: var(--text-faint); font-size: var(--sc-t-meta); padding: 4px 8px; }
  td { padding: 6px 8px; border-top: 1px solid var(--border); vertical-align: top; font-size: var(--sc-t-body); }
  .ok { color: var(--ok); }
  .warn { color: var(--warn-strong); }
  .error { color: var(--error); }
  .accent { color: var(--accent); }
  .label { display: inline-block; margin: 0 6px 4px 0; padding: 0 6px; border: 1px solid var(--border); border-radius: var(--radius-sm); font-size: 12px; }
  .note { margin: 0 0 12px; color: var(--text-dim); font-size: var(--sc-t-meta); }
  .yamlbar { display: flex; justify-content: flex-end; margin-bottom: 6px; }
  .danger:not(:disabled) { color: var(--error); }
  .dim { color: var(--text-faint); }
</style>
