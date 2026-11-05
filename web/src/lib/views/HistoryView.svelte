<script>
  // This node's record of itself (#121, stormcos#456): its boots, installs,
  // drives' health over time and hardware, kept in system-data across every
  // install. Read when opened, from the files: nothing here is a copy.
  //
  // Which part and which record travel in the URL, so a boot can be linked.
  import { get } from '../api.js'
  import { route, navigate } from '../router.svelte.js'
  import { ago } from '../ui/time.js'
  import { kernelLines, timelineRows, slowestPhases, counterTrends, findings, assetChange, assetGroups, bytes, seconds, bootEnd } from '../history.js'
  import PageHeader from '../components/PageHeader.svelte'
  import EmptyState from '../components/EmptyState.svelte'
  import StatusPill from '../components/StatusPill.svelte'
  import CopyButton from '../components/CopyButton.svelte'

  const API = '/api/v1/history'
  const TABS = [
    ['boots', 'Boots'],
    ['installs', 'Installs'],
    ['drives', 'Drives'],
    ['assets', 'Hardware'],
  ]
  const tab = route.current.query.get('tab') || 'boots'
  const id = route.current.query.get('id') || ''
  const link = (t, i) => `#/history?tab=${t}${i ? `&id=${encodeURIComponent(i)}` : ''}`

  let status = $state(null)
  let list = $state(null)
  let crashes = $state(null)
  let record = $state(null)
  let error = $state('')

  async function read(path) {
    const resp = await fetch(`${API}${path}`)
    const body = await resp.json().catch(() => ({}))
    if (!resp.ok) throw new Error(body.error || `${resp.status} ${resp.statusText}`)
    return body
  }

  async function load() {
    try {
      status = await get(API)
      if (!status.available) return
      if (id) {
        record = await read(`/${tab}/${encodeURIComponent(id)}`)
      } else {
        list = await read(`/${tab}`)
        if (tab === 'boots') crashes = await read('/crashes')
      }
    } catch (e) {
      error = e.message
    }
  }
  load()

  const when = (t) => (t ? `${ago(t)} ago` : '—')
  // A record's time is a stamp in its name when it is stormblock's or stormdrive's.
  function stampIso(s) {
    const m = /^(\d{4})(\d\d)(\d\d)T(\d\d)(\d\d)(\d\d)Z/.exec(s || '')
    return m ? `${m[1]}-${m[2]}-${m[3]}T${m[4]}:${m[5]}:${m[6]}Z` : s
  }
  // A boot id reads by its first part; a drive's key or a stamp is the whole name.
  const short = (s) => (/^[0-9a-f]{8}-[0-9a-f]{4}-/.test(s || '') ? s.slice(0, 8) : s)

  // --- a boot's kernel log: the last of it, and all of it to download.
  const TAIL = 5000
  let kernelText = $state(null)
  let kernelError = $state('')
  let kernelLevel = $state(6)
  let kernelSearch = $state('')
  async function openKernel() {
    kernelError = ''
    try {
      const resp = await fetch(`${API}/boots/${encodeURIComponent(id)}/kernel?tail=${TAIL}`)
      if (!resp.ok) throw new Error((await resp.json().catch(() => ({}))).error || `${resp.status}`)
      kernelText = await resp.text()
    } catch (e) {
      kernelError = e.message
    }
  }
  const kernelRows = $derived(kernelLines(kernelText, kernelLevel, kernelSearch))

  let crashText = $state(null)
  async function openCrash(dir, file) {
    const resp = await fetch(`${API}/crashes/${encodeURIComponent(dir)}/${encodeURIComponent(file)}`)
    crashText = { file, text: await resp.text() }
  }
</script>

<div class="sc-page">
  <PageHeader
    crumbs={[{ label: 'Compute' }, { label: 'History', href: '#/history' }, ...(id ? [{ label: TABS.find((t) => t[0] === tab)?.[1] || tab, href: link(tab) }] : [])]}
    title={id ? (tab === 'assets' ? 'Hardware record' : short(id)) : 'History'}
    scope="this node"
  />

  {#if status && !status.available}
    <EmptyState icon="history" title="No system-data on this node" hint={status.note} />
  {:else if status}
    <p class="dim small">
      Kept across every install in <span class="mono">{status.root}</span>; a node reset is the only thing that clears it.
      Other nodes keep their own: open that node's console for its history.
    </p>
    <nav class="tabs" aria-label="History sections">
      {#each TABS as [t, label]}
        <button class:active={tab === t} onclick={() => navigate(link(t))}>
          {label} <span class="n">{status[t]}</span>
        </button>
      {/each}
    </nav>

    {#if error}<p class="error">{error}</p>{/if}

    {#if !id && list}
      {#if list.note}<p class="dim">{list.note}</p>{/if}
      {#each list.unreadable || [] as u}<p class="warn small">Could not be read: {u}</p>{/each}
      {#if list.total > list.items.length}<p class="dim small">The newest {list.items.length} of {list.total}.</p>{/if}
    {/if}

    <!-- Boots -->
    {#if tab === 'boots' && !id && list}
      {#if crashes?.items?.length}
        <section class="card">
          <h2>Kernel crashes</h2>
          <table>
            <thead><tr><th>When</th><th>Panic</th><th>Release</th><th>Kernel</th><th>Boot</th></tr></thead>
            <tbody>
              {#each crashes.items as c}
                <tr class="alert">
                  <td class="num">{c.manifest.record_unix ? when(new Date(c.manifest.record_unix * 1000).toISOString()) : '—'}</td>
                  <td>{c.manifest.panic || '—'}</td>
                  <td>{c.manifest.release || '—'}</td>
                  <td class="mono">{c.manifest.kernel || '—'}</td>
                  <td>{#if c.manifest.boot_id}<a href={link('boots', c.manifest.boot_id)}>{short(c.manifest.boot_id)}</a>{:else}<span class="dim mono">{c.dir}</span>{/if}</td>
                </tr>
              {/each}
            </tbody>
          </table>
        </section>
      {/if}
      {#if list.items.length}
        <div class="table-wrap">
          <table>
            <thead>
              <tr><th>When</th><th>Boot</th><th>Release</th><th>Kernel</th><th>Ended</th><th>All running</th><th>Services</th><th>Disk</th><th>Kernel log</th></tr>
            </thead>
            <tbody>
              {#each list.items as b (b.id)}
                {@const e = bootEnd(b)}
                <tr class:alert={b.end === 'crashed'}>
                  <td class="num" title={b.at || ''}>{when(b.at)}</td>
                  <td><a class="mono" href={link('boots', b.id)}>{short(b.id)}</a>{#if b.current} <span class="here">this boot</span>{/if}</td>
                  <td>{b.release || '—'}{#if b.installed} <span class="tag">installed</span>{/if}</td>
                  <td class="mono">{b.kernel || '—'}</td>
                  <td title={b.end_detail || ''}><StatusPill health={e.health} label={e.label} /></td>
                  <td class="num">{seconds(b.all_running_ms)}</td>
                  <td class="num">
                    {#if b.events}{b.started} started{#if b.exited}{' · '}<span class="warn">{b.exited} exited</span>{/if}{#if b.refused}{' · '}<span class="bad">{b.refused} refused</span>{/if}{:else}—{/if}
                  </td>
                  <td>{b.disk || '—'}</td>
                  <td class="num">{bytes(b.kernel_log_bytes)}</td>
                </tr>
              {/each}
            </tbody>
          </table>
        </div>
      {/if}
    {/if}

    {#if tab === 'boots' && id && record}
      {@const e = bootEnd(record)}
      <section class="card">
        <div class="cardhead"><h2>This boot</h2><StatusPill health={e.health} label={e.label} /></div>
        {#if record.end_detail}<p class:bad={record.end === 'crashed'}>{record.end_detail}</p>{/if}
        <ul class="kv">
          <li><span class="k">boot id</span> <span class="mono">{record.boot_id || 'not recorded: PID 1 kept no timeline for this boot'}</span></li>
          <li><span class="k">when</span> <span title={record.at || ''}>{when(record.at)}</span></li>
          <li><span class="k">release</span> {record.release || '—'}{#if record.installed}{` · installed ${record.installed}`}{/if}</li>
          <li><span class="k">kernel</span> <span class="mono">{record.kernel || '—'}</span></li>
          <li><span class="k">host</span> {record.hostname || record.tag || '—'}</li>
          <li><span class="k">all running</span> {seconds(record.all_running_ms)}{#if record.pid1_starts > 1}{` · PID 1 started ${record.pid1_starts} times`}{/if}</li>
          <li><span class="k">command line</span> <span class="mono wrap">{record.cmdline || '—'}</span> {#if record.cmdline}<CopyButton value={record.cmdline} />{/if} {#if record.cmdline}<span class="dim small">({record.cmdline.length} bytes)</span>{/if}</li>
          {#if record.assets}<li><span class="k">hardware</span> <a href={link('assets', record.assets)}>this boot's record</a></li>{/if}
        </ul>
        {#if record.stormblock_file && record.joined === 'time'}
          <p class="dim small">stormblock's record <span class="mono">{record.stormblock_file}</span> names no boot; it is shown here because its time and command line match.</p>
        {/if}
      </section>

      {#if record.crash}
        <section class="card">
          <h2>Kernel crash</h2>
          <p class="bad">{record.crash.manifest.panic || 'a panic with no line kept'}</p>
          <div class="row">
            {#each record.crash.files as f}
              <button onclick={() => openCrash(record.crash.dir, f)}>{f}</button>
            {/each}
          </div>
          {#if crashText}<h3>{crashText.file}</h3><pre class="log">{crashText.text}</pre>{/if}
        </section>
      {/if}

      {#if record.stormblock?.local_disk}
        <section class="card">
          <h2>The disk, as stormblock found it</h2>
          <ul class="kv">
            <li><span class="k">verdict</span> {record.stormblock.local_disk.state || '—'}</li>
            <li><span class="k">drive</span> <span class="mono">{record.stormblock.local_disk.drive || '—'}</span></li>
            <li><span class="k">install config</span> {record.stormblock.install_config || '—'}</li>
          </ul>
          <details><summary>The whole record</summary><pre class="log">{JSON.stringify(record.stormblock, null, 2)}</pre></details>
        </section>
      {:else if record.stormblock}
        <section class="card"><h2>stormblock's record</h2><pre class="log">{JSON.stringify(record.stormblock, null, 2)}</pre></section>
      {/if}

      <section class="card">
        <h2>Timeline</h2>
        {#if !record.timeline.length}
          <p class="dim">PID 1 kept no timeline for this boot.</p>
        {:else}
          {@const slow = slowestPhases(record.timeline)}
          {#if slow.length}
            <p class="small"><span class="k">slowest</span> {#each slow as p, i}{i ? ' · ' : ''}{p.name} <span class="mono">{seconds(p.ms)}</span>{/each}</p>
          {/if}
          {#if record.timeline_cut}<p class="warn small">Only the end of this timeline was read.</p>{/if}
          <div class="table-wrap">
            <table>
              <thead><tr><th>Since boot</th><th>Event</th><th>Name</th><th>Took</th><th>Detail</th></tr></thead>
              <tbody>
                {#each timelineRows(record.timeline) as r}
                  <tr class:alert={r.tone === 'bad'}>
                    <td class="num">{r.event === 'crashed' && !r.t_ms ? '—' : seconds(r.t_ms)}</td>
                    <td class:bad={r.tone === 'bad'} class:warn={r.tone === 'warn'}>{r.event}</td>
                    <td>{r.name || ''}</td>
                    <td class="num">{r.ms != null ? seconds(r.ms) : ''}</td>
                    <td class="err">{r.detail || ''}</td>
                  </tr>
                {/each}
              </tbody>
            </table>
          </div>
        {/if}
      </section>

      <section class="card">
        <div class="cardhead">
          <h2>Kernel log</h2>
          {#if record.kernel_log_bytes != null}
            <span class="row">
              <span class="dim small">{bytes(record.kernel_log_bytes)}</span>
              <a class="btn" href="{API}/boots/{encodeURIComponent(id)}/kernel?download=true" download>Download</a>
            </span>
          {/if}
        </div>
        {#if record.kernel_log_bytes == null}
          <p class="dim">No kernel log kept for this boot. PID 1 keeps the last 32.</p>
        {:else if kernelText === null}
          <button onclick={openKernel}>Show the last {TAIL} lines</button>
          {#if kernelError}<p class="error">{kernelError}</p>{/if}
        {:else}
          <div class="row bar">
            <label>Level
              <select bind:value={kernelLevel}>
                <option value={3}>errors</option>
                <option value={4}>warnings</option>
                <option value={6}>info</option>
                <option value={7}>everything</option>
              </select>
            </label>
            <input bind:value={kernelSearch} placeholder="Search this log" aria-label="Search this log" />
            <span class="dim small">{kernelRows.length} lines</span>
          </div>
          <div class="log kernel">
            {#each kernelRows as l}
              <div class="kl {l.level}"><span class="t">{l.time ?? ''}</span><span class="lv">{l.gap ? '' : l.level}</span><span>{l.text}</span></div>
            {/each}
            {#if !kernelRows.length}<p class="dim">Nothing at this level.</p>{/if}
          </div>
        {/if}
      </section>
    {/if}

    <!-- Installs -->
    {#if tab === 'installs' && !id && list?.items.length}
      <div class="table-wrap">
        <table>
          <thead><tr><th>When</th><th>Release</th><th>Over</th><th>Disk</th><th>Drive</th><th>Carried</th><th>Install config</th><th>Record</th></tr></thead>
          <tbody>
            {#each list.items as i (i.id)}
              <tr>
                <td class="num" title={i.at || ''}>{when(i.at)}</td>
                <td>{i.version || i.release || '—'}</td>
                <td>{i.previous || '—'}</td>
                <td>{i.disk || '—'}</td>
                <td class="mono">{i.drive || '—'}</td>
                <td class="num">{Array.isArray(i.carried) ? i.carried.length : '—'}</td>
                <td>{i.install_config || '—'}</td>
                <td><a class="mono" href={link('installs', i.id)}>{i.id}</a> · <a href={link('boots', i.id)}>its boot</a></td>
              </tr>
            {/each}
          </tbody>
        </table>
      </div>
    {/if}
    {#if tab === 'installs' && id && record}
      <section class="card">
        <div class="cardhead"><h2>Install {id}</h2><a href={link('boots', id)}>Its boot</a></div>
        <pre class="log">{JSON.stringify(record, null, 2)}</pre>
      </section>
    {/if}

    <!-- Drives -->
    {#if tab === 'drives' && !id && list?.items.length}
      <div class="table-wrap">
        <table>
          <thead><tr><th>Drive</th><th>Serial</th><th>Model</th><th>Bay</th><th>Size</th><th>Status</th><th>Temp</th><th>Last record</th><th>Findings</th><th>Last format</th></tr></thead>
          <tbody>
            {#each list.items as d (d.key)}
              <tr class:alert={d.findings?.length}>
                <td><a class="mono" href={link('drives', d.key)}>{d.key}</a></td>
                <td class="mono">{d.drive?.serial || '—'}</td>
                <td>{d.drive?.model || '—'}</td>
                <td>{d.drive?.bay || '—'}</td>
                <td class="num">{bytes(d.drive?.capacity_bytes)}</td>
                <td>{d.status || '—'}</td>
                <td class="num">{d.temperature_c != null ? `${d.temperature_c} °C` : '—'}</td>
                <td class="num" title={d.last_at || ''}>{when(d.last_at)}</td>
                <td class:bad={d.findings?.length}>{d.findings?.length || ''}</td>
                <td class="small">{d.summary?.last_format ? `${d.summary.last_format.result} ${when(d.summary.last_format.at)}` : '—'}</td>
              </tr>
            {/each}
          </tbody>
        </table>
      </div>
    {/if}
    {#if tab === 'drives' && id && record}
      {@const newest = record.records[0]}
      {@const found = findings(record.records)}
      <section class="card">
        <h2>The drive</h2>
        <ul class="kv">
          <li><span class="k">model</span> {newest?.drive?.model || '—'}</li>
          <li><span class="k">serial</span> <span class="mono">{newest?.drive?.serial || '—'}</span></li>
          <li><span class="k">wwn</span> <span class="mono">{newest?.drive?.wwn || '—'}</span></li>
          <li><span class="k">firmware</span> {newest?.drive?.firmware || '—'}</li>
          <li><span class="k">where</span> {newest?.drive?.bay || '—'} · <span class="mono">{newest?.drive?.path || '—'}</span>{#if record.summary?.nodes?.length}{` · seen on ${record.summary.nodes.join(', ')}`}{/if}</li>
          <li><span class="k">records</span> {record.records.length}{#if record.summary}{` · ${record.summary.actions} operations, ${record.summary.warnings} warnings since ${record.summary.first_at}`}{/if}</li>
        </ul>
        {#if record.cut}<p class="warn small">Only the end of a long month was read.</p>{/if}
      </section>
      {#if found.length}
        <section class="card">
          <h2>Findings</h2>
          <ul class="plain">{#each found as f}<li class:bad={f.bad} class:warn={!f.bad}><span class="dim num" title={f.at}>{when(f.at)}</span> {f.message}</li>{/each}</ul>
        </section>
      {/if}
      <section class="card">
        <h2>Counters over these records</h2>
        <div class="table-wrap">
          <table>
            <thead><tr><th>Counter</th><th>First</th><th>Latest</th><th>Moved</th></tr></thead>
            <tbody>
              {#each counterTrends(record.records) as c (c.name)}
                <tr class:alert={c.back}>
                  <td class="mono">{c.name}</td>
                  <td class="num">{c.first}</td>
                  <td class="num">{c.latest}</td>
                  <td class="num" class:bad={c.back}>{c.delta === 0 ? '' : `${c.delta > 0 ? '+' : ''}${c.delta}`}</td>
                </tr>
              {/each}
            </tbody>
          </table>
        </div>
      </section>
      <section class="card">
        <h2>What happened to it</h2>
        {#if !record.log.length}<p class="dim">Nothing recorded yet.</p>{:else}
          <table>
            <thead><tr><th>When</th><th>Kind</th><th>What</th><th>Node</th></tr></thead>
            <tbody>
              {#each record.log as e}
                <tr class:alert={e.severity === 'error'}>
                  <td class="num" title={e.at}>{when(e.at)}</td>
                  <td>{e.kind}</td>
                  <td class:warn={e.severity === 'warning'} class:bad={e.severity === 'error'}>{e.message}</td>
                  <td>{e.node || ''}</td>
                </tr>
              {/each}
            </tbody>
          </table>
        {/if}
      </section>
    {/if}

    <!-- Hardware -->
    {#if tab === 'assets' && !id && list?.items.length}
      <div class="table-wrap">
        <table>
          <thead><tr><th>When</th><th>Node</th><th>Boot</th><th>Kernel</th><th>Memory</th><th>Parts</th><th>What changed</th></tr></thead>
          <tbody>
            {#each list.items as a (a.id)}
              {@const changes = [...(a.changes || []), ...(a.changed_in_boot || [])]}
              <tr>
                <td class="num" title={a.at || ''}><a href={link('assets', a.id)}>{when(stampIso(a.at))}</a></td>
                <td>{a.node || '—'}</td>
                <td>{#if a.boot_id}<a class="mono" href={link('boots', a.boot_id)}>{short(a.boot_id)}</a>{:else}—{/if}</td>
                <td class="mono">{a.kernel || '—'}</td>
                <td class="num">{bytes(a.memory_total_bytes)}</td>
                <td class="num">{a.items}</td>
                <td class="small" class:warn={changes.length}>{changes.map(assetChange).join('; ') || (a.previous ? 'nothing' : 'the first record')}</td>
              </tr>
            {/each}
          </tbody>
        </table>
      </div>
    {/if}
    {#if tab === 'assets' && id && record}
      {@const changes = [...(record.changes || []), ...(record.changed_in_boot || [])]}
      <section class="card">
        <h2>This boot's hardware</h2>
        <ul class="kv">
          <li><span class="k">node</span> {record.node || '—'}</li>
          <li><span class="k">boot</span> <a class="mono" href={link('boots', record.boot_id)}>{short(record.boot_id)}</a></li>
          <li><span class="k">memory</span> {bytes(record.memory_total_bytes)}</li>
          <li><span class="k">compared with</span> {#if record.previous}<a class="mono" href={link('assets', record.previous.replace(/\.json$/, ''))}>{record.previous}</a>{:else}nothing: the first record{/if}</li>
        </ul>
        {#if changes.length}
          <h3>What changed</h3>
          <ul class="plain">{#each changes as c}<li class="warn">{assetChange(c)}</li>{/each}</ul>
        {/if}
      </section>
      {#each assetGroups(record.items) as g (g.group)}
        <section class="card">
          <h2>{g.group}</h2>
          <table>
            <tbody>
              {#each g.rows as r (r.key)}
                <tr>
                  <td class="mono name">{r.name}</td>
                  <td class="small">{#each r.fields as [k, v], i}{i ? ' · ' : ''}{#if k}<span class="dim">{k}&nbsp;</span>{/if}{v}{/each}</td>
                </tr>
              {/each}
            </tbody>
          </table>
        </section>
      {/each}
    {/if}
  {/if}
</div>

<style>
  .small { font-size: var(--sc-t-meta); }
  .dim { color: var(--text-dim); }
  .warn { color: var(--warn-strong); }
  .error, .bad { color: var(--error); }
  .mono { font-family: var(--mono); font-size: var(--sc-t-meta); }
  .wrap { word-break: break-all; }
  .here, .tag { font-size: var(--sc-t-eyebrow); padding: 1px 6px; border-radius: 999px; border: 1px solid var(--border); color: var(--text-dim); white-space: nowrap; }
  .tabs { display: flex; gap: 2px; border-bottom: 1px solid var(--border); margin: 10px 0 14px; }
  .tabs button { background: none; border: 0; border-bottom: 2px solid transparent; border-radius: 0; padding: 8px 14px; color: var(--text-dim); cursor: pointer; font: inherit; }
  .tabs button:hover { color: var(--text); background: var(--nav-hover); }
  .tabs button.active { color: var(--text); border-bottom-color: var(--accent); font-weight: 600; }
  .tabs .n { color: var(--text-faint); font-variant-numeric: tabular-nums; font-size: var(--sc-t-meta); margin-left: 4px; }
  .table-wrap { overflow-x: auto; }
  table { width: 100%; border-collapse: collapse; background: var(--panel); border: 1px solid var(--border); border-radius: var(--radius); margin-bottom: 12px; font-size: var(--sc-t-body); }
  th { text-align: left; font-size: var(--sc-t-eyebrow); text-transform: uppercase; letter-spacing: 0.05em; color: var(--text-faint); padding: 8px 12px; border-bottom: 1px solid var(--border); }
  td { padding: 6px 12px; vertical-align: top; }
  tr + tr > td { border-top: 1px solid var(--sc-hairline); }
  tr.alert td:first-child { box-shadow: inset 3px 0 var(--error); }
  td.num { font-variant-numeric: tabular-nums; font-family: var(--mono); font-size: var(--sc-t-meta); white-space: nowrap; }
  td.err { color: var(--text-dim); font-size: var(--sc-t-meta); max-width: 520px; overflow-wrap: anywhere; }
  td.name { white-space: nowrap; }
  .card { background: var(--panel); border: 1px solid var(--border); border-radius: var(--radius); padding: 14px var(--sc-row-px); margin-bottom: 12px; min-width: 0; }
  .card table { border: 0; margin-bottom: 0; }
  .cardhead { display: flex; justify-content: space-between; align-items: center; margin-bottom: 8px; gap: 12px; }
  h2 { font-size: var(--sc-t-eyebrow); text-transform: uppercase; letter-spacing: 0.06em; color: var(--text-faint); margin: 0 0 8px; }
  .cardhead h2 { margin: 0; }
  h3 { font-size: var(--sc-t-meta); margin: 12px 0 4px; }
  ul.kv { list-style: none; margin: 0; padding: 0; display: grid; gap: 4px; }
  .k { display: inline-block; min-width: 120px; color: var(--text-faint); font-size: var(--sc-t-meta); }
  ul.plain { list-style: none; margin: 0; padding: 0; display: grid; gap: 4px; }
  .row { display: flex; gap: 8px; align-items: center; flex-wrap: wrap; }
  .bar { margin-bottom: 8px; }
  .bar input { width: 280px; max-width: 100%; }
  .log { font-family: var(--mono); font-size: var(--sc-t-meta); background: var(--bg); border: 1px solid var(--border); border-radius: var(--radius); padding: 8px; max-height: 560px; overflow: auto; margin: 8px 0 0; white-space: pre-wrap; overflow-wrap: anywhere; }
  .kl { display: flex; gap: 10px; }
  .kl .t { color: var(--text-faint); min-width: 90px; text-align: right; flex: none; }
  .kl .lv { color: var(--text-faint); min-width: 56px; flex: none; }
  .kl.emerg, .kl.alert, .kl.crit, .kl.err { color: var(--error); }
  .kl.warning { color: var(--warn-strong); }
  .kl.gap { color: var(--warn-strong); font-style: italic; }
</style>
