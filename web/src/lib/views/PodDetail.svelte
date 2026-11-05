<script>
  // One pod, all of it (#69): what it runs and from which image, where,
  // on which addresses, behind which Services, what it is sending, and
  // its logs — the current run, the one before, and the runs the console
  // kept.
  //
  // What the node does not report is said where it would be, with the
  // issue that would fill it: a blank reads as "nothing there", and an
  // image with no digest is not an image with no digest.
  import { onDestroy } from 'svelte'
  import { route } from '../router.svelte.js'
  import { feed } from '../stores.svelte.js'
  import { get, call, formatBytes } from '../api.js'
  import { ago } from '../ui/time.js'
  import PageHeader from '../components/PageHeader.svelte'
  import EmptyState from '../components/EmptyState.svelte'
  import StatusPill from '../components/StatusPill.svelte'
  import YamlPanel from '../components/YamlPanel.svelte'
  import EventBox from '../components/EventBox.svelte'
  import CopyButton from '../components/CopyButton.svelte'
  import PodLogs from './PodLogs.svelte'
  import PodStats from './PodStats.svelte'

  const ns = $derived(route.current.params.ns)
  const name = $derived(route.current.params.name)
  const TABS = ['Overview', 'Stats', 'Network', 'Logs', 'Events', 'YAML']
  let tab = $state(TABS.includes(route.current.query.get('tab')) ? route.current.query.get('tab') : 'Overview')
  // One click to a container's logs, or its previous instance (#167).
  let logContainer = $state(route.current.query.get('container') || '')
  let logSource = $state(route.current.query.get('source') || 'current')
  function openLogs(container, source = 'current') {
    logContainer = container
    logSource = source
    tab = 'Logs'
  }
  /// One line per env var: what `describe` prints.
  function envLine(e) {
    if (e.from === 'secret') return `from Secret ${e.ref}, key ${e.key}${e.optional ? ' (optional)' : ''}: value not shown`
    if (e.from === 'configMap') return `from ConfigMap ${e.ref}, key ${e.key}${e.optional ? ' (optional)' : ''}`
    if (e.from === 'field') return `from the pod: ${e.ref}`
    if (e.from === 'resource') return `from its resources: ${e.ref}`
    return e.value
  }
  /// Where a container's image is in the registry: its digest, else its repository.
  function registryHref(c) {
    const repo = c.digest || c.image.replace(/^stormpump:\/\//, '').split('@')[0].split(':')[0]
    return `#/registry?${new URLSearchParams({ search: repo })}`
  }

  let pod = $state(null)
  let yaml = $state('')
  let error = $state('')
  let loaded = $state(false)

  const base = $derived(`/api/plugins/k8s/pods/${encodeURIComponent(ns)}/${encodeURIComponent(name)}`)
  const component = $derived(feed.components.find((c) => c.id === `k8s:pod:${ns}/${name}`))

  async function load() {
    try {
      pod = await get(base)
      error = ''
    } catch (e) {
      error = e.message
    }
    loaded = true
  }

  async function loadYaml() {
    try {
      const o = await get(`/api/plugins/k8s/object/pod/${encodeURIComponent(ns)}/${encodeURIComponent(name)}`)
      yaml = o.yaml
    } catch (e) {
      yaml = `# ${e.message}`
    }
  }

  // The page refreshes on its own: restarts, readiness and kept runs move.
  load()
  const timer = setInterval(load, 10000)
  onDestroy(() => clearInterval(timer))

  $effect(() => {
    if (tab === 'YAML') loadYaml()
  })

  // ---- traffic: counters from the node's kubelet, rates between reads
  const SAMPLES = 60
  let traffic = $state(null)
  let history = $state({}) // interface -> [{at, rx, tx}]
  let trafficTimer = null

  async function readTraffic() {
    try {
      const t = await get(`${base}/traffic`)
      traffic = t
      if (t.available) {
        const next = { ...history }
        for (const i of t.interfaces) {
          const h = (next[i.interface] || []).concat([{ at: t.at, rx: i.rxBytes ?? 0, tx: i.txBytes ?? 0 }])
          next[i.interface] = h.slice(-SAMPLES)
        }
        history = next
      }
    } catch (e) {
      traffic = { available: false, reason: e.message }
    }
  }

  $effect(() => {
    if (tab === 'Network' || tab === 'Overview') {
      if (!trafficTimer) {
        readTraffic()
        trafficTimer = setInterval(readTraffic, 5000)
      }
    } else if (trafficTimer) {
      clearInterval(trafficTimer)
      trafficTimer = null
    }
  })
  onDestroy(() => trafficTimer && clearInterval(trafficTimer))

  // Bytes a second between consecutive reads; a counter that went
  // backwards (the pod's sandbox was recreated) starts over rather than
  // drawing a negative rate.
  function rates(h, key) {
    const out = []
    for (let i = 1; i < h.length; i++) {
      const dt = (h[i].at - h[i - 1].at) / 1000
      const d = h[i][key] - h[i - 1][key]
      out.push(dt > 0 && d >= 0 ? d / dt : 0)
    }
    return out
  }
  function spark(values, w = 160, hgt = 28) {
    if (values.length < 2) return ''
    const max = Math.max(...values, 1)
    return values
      .map((v, i) => `${((i / (values.length - 1)) * w).toFixed(1)},${(hgt - (v / max) * (hgt - 2) - 1).toFixed(1)}`)
      .join(' ')
  }
  const rate = (h, key) => {
    const r = rates(h || [], key)
    return r.length ? r[r.length - 1] : null
  }
  const perSec = (v) => (v === null ? '—' : `${formatBytes(v)}/s`)

  // ---- events: kubectl describe's table, followed every 5 s on its tab
  let events = $state(null)
  let eventsTimer = null
  async function readEvents() {
    try {
      events = await get(`${base}/events`)
    } catch (e) {
      events = { available: false, reason: e.message, items: [] }
    }
  }
  $effect(() => {
    if (tab === 'Events') {
      if (!eventsTimer) {
        readEvents()
        eventsTimer = setInterval(readEvents, 5000)
      }
    } else if (eventsTimer) {
      clearInterval(eventsTimer)
      eventsTimer = null
    }
  })
  onDestroy(() => eventsTimer && clearInterval(eventsTimer))

  // ---- probes and resources, as describe prints them
  const PROBES = ['startup', 'liveness', 'readiness']
  const probeRows = $derived(
    (pod?.containers || []).flatMap((c) => PROBES.filter((k) => c.probes?.[k]).map((k) => ({ c, kind: k, p: c.probes[k] }))),
  )
  const probeTone = (r) => ({ passing: 'ok', passed: 'ok', failing: 'warn' })[r] || ''
  const resLine = (r) => Object.entries(r || {}).map(([k, v]) => `${k} ${v}`).join(', ')

  // ---- small helpers
  const kv = (o) => Object.entries(o || {}).sort(([a], [b]) => a.localeCompare(b))
  function stateLine(st) {
    if (!st) return '—'
    const bits = [st.kind]
    if (st.reason) bits.push(st.reason)
    if (st.exitCode !== null && st.exitCode !== undefined) bits.push(`exit ${st.exitCode}`)
    return bits.join(' · ')
  }
  const short = (d) => (d ? `${d.slice(0, 19)}…` : '')
  // `from rustkube-node@4263d80, …` — what the registry image was built from.
  const sourcesLine = (g) => {
    const parts = Object.entries(g?.sources || {}).map(([repo, commit]) => `${repo}@${String(commit).slice(0, 7)}`)
    return parts.length ? ` from ${parts.join(', ')}` : ''
  }
  const builtAt = (g) => (g?.builtAt ? new Date(g.builtAt * 1000).toISOString().replace('.000', '') : '')
  const gapFor = (what) => pod?.gaps?.find((g) => g.what === what)
  const invoke = (a) => call(a.method, a.path)
  const deleteAction = $derived(component?.actions?.find((a) => a.id === 'delete'))
</script>

<div class="sc-page">
  <PageHeader
    crumbs={[{ label: 'Projects', href: '#/projects' }, { label: ns, href: `#/k8s/ns/${encodeURIComponent(ns)}` }, { label: 'Pods', href: `#/k8s/pod?ns=${encodeURIComponent(ns)}` }, { label: name }]}
    title={name}
    scope={pod?.metadata?.created ? `created ${ago(pod.metadata.created)} ago` : ''}
  >
    {#snippet status()}
      {#if component}<StatusPill health={component.health} />{/if}
    {/snippet}
    {#snippet actions()}
      {#if deleteAction}
        <button class="danger" onclick={() => confirm(`Delete pod ${ns}/${name}?`) && invoke(deleteAction)}>Delete</button>
      {/if}
    {/snippet}
  </PageHeader>

  {#if !loaded}
    <div class="sc-empty"><p>Loading {name}…</p></div>
  {:else if error && !pod}
    <EmptyState icon="cluster" title="This pod cannot be read" hint="The kubernetes plugin returned: {error}">
      {#snippet action()}
        <a class="sc-back" href="#/k8s/pod">Back to pods</a>
      {/snippet}
    </EmptyState>
  {:else}
    {#if error}<p class="stale">Showing the last answer — the refresh failed: {error}</p>{/if}
    <nav class="tabs" aria-label="Pod sections">
      {#each TABS as t}
        <button class:active={tab === t} aria-current={tab === t ? 'page' : undefined} onclick={() => (tab = t)}>{t}</button>
      {/each}
    </nav>

    {#if tab === 'Overview'}
      <section class="cards">
        <div class="card">
          <h2>Pod</h2>
          <dl>
            <dt>Phase</dt><dd>{pod.metadata.phase || '—'}{#if pod.metadata.reason} · {pod.metadata.reason}{/if}</dd>
            <dt>Node</dt>
            <dd class="mono">{#if pod.metadata.node}<a href={`#/grid?id=k8s:node:${encodeURIComponent(pod.metadata.node)}`}>{pod.metadata.node}</a>{:else}not scheduled{/if}</dd>
            <dt>Pod IP</dt>
            <dd class="mono">{#each pod.network.podIPs as ip (ip)}<div>{ip}<CopyButton value={ip} label="Copy address" /></div>{:else}{pod.network.hostNetwork ? 'the node’s (host network)' : '—'}{/each}</dd>
            <dt>Host IP</dt><dd class="mono">{pod.network.hostIP || '—'}{#if pod.network.hostNetwork} <span class="dim">(host network)</span>{/if}</dd>
            <dt>Created</dt><dd title={pod.metadata.created}>{pod.metadata.created ? `${ago(pod.metadata.created)} ago` : '—'}</dd>
            <dt>Started</dt><dd>{pod.metadata.startTime ? `${ago(pod.metadata.startTime)} ago` : '—'}</dd>
            <dt>QoS</dt><dd>{pod.metadata.qosClass}</dd>
            <dt>Priority</dt><dd>{pod.metadata.priority ?? '—'}{#if pod.metadata.priorityClassName} · {pod.metadata.priorityClassName}{/if}</dd>
            <dt>Service account</dt><dd class="mono">{pod.metadata.serviceAccount || '—'}</dd>
            <dt>Restart policy</dt><dd>{pod.metadata.restartPolicy}</dd>
            {#if Object.keys(pod.metadata.nodeSelector || {}).length}<dt>Node selector</dt><dd class="mono">{Object.entries(pod.metadata.nodeSelector).map(([k, v]) => `${k}=${v}`).join(', ')}</dd>{/if}
            {#if pod.metadata.tolerations?.length}
              <dt>Tolerations</dt>
              <dd class="mono">{#each pod.metadata.tolerations as t, i (i)}<div>{t.key || '(all)'}{t.operator === 'Exists' ? ' exists' : t.value ? `=${t.value}` : ''}{t.effect ? `:${t.effect}` : ''}{t.tolerationSeconds != null ? ` for ${t.tolerationSeconds}s` : ''}</div>{/each}</dd>
            {/if}
            <dt>Owned by</dt>
            <dd>
              {#each pod.owners as o, i (i)}
                {#if i > 0}<span class="dim"> ← </span>{/if}
                {#if o.href}<a href={o.href}>{o.kind} {o.name}</a>{:else}{o.kind} {o.name}{/if}
              {:else}
                {pod.metadata.mirror ? 'the node (a static pod mirrored to the apiserver)' : 'nothing — a bare pod'}
              {/each}
            </dd>
          </dl>
          {#if pod.metadata.message}<p class="reason">{pod.metadata.message}</p>{/if}
        </div>

        <div class="card">
          <h2>Conditions</h2>
          {#if !pod.metadata.conditions.length}
            <p class="none">The node has reported no conditions.</p>
          {:else}
            <table>
              <tbody>
                {#each pod.metadata.conditions as c (c.type)}
                  <tr>
                    <td>{c.type}</td>
                    <td class:ok={c.status === 'True'} class:warn={c.status !== 'True'}>{c.status}</td>
                    <td class="dim" title={c.lastTransitionTime}>{c.lastTransitionTime ? `${ago(c.lastTransitionTime)} ago` : ''}</td>
                  </tr>
                  {#if c.reason || c.message}<tr class="why"><td colspan="3">{c.reason || ''} {c.message || ''}</td></tr>{/if}
                {/each}
              </tbody>
            </table>
          {/if}
        </div>

        <div class="card">
          <h2>Traffic</h2>
          {#if !traffic}
            <p class="none">Reading the node…</p>
          {:else if !traffic.available}
            <p class="none">{traffic.reason}</p>
          {:else}
            {#each traffic.interfaces as i (i.interface)}
              <div class="iface">
                <span class="mono">{i.interface}</span>
                <span>↓ {perSec(rate(history[i.interface], 'rx'))}</span>
                <span>↑ {perSec(rate(history[i.interface], 'tx'))}</span>
              </div>
            {/each}
            <p class="dim"><button class="link" onclick={() => (tab = 'Network')}>Counters and history on Network</button></p>
          {/if}
        </div>
      </section>

      <section class="card wide">
        <h2>Containers</h2>
        <div class="table-wrap">
          <table class="containers">
            <thead>
              <tr><th>Name</th><th>State</th><th title="ready / started">Ready</th><th>Restarts</th><th>Last termination</th><th>Requests</th><th>Limits</th><th>Image</th><th>Ports</th><th></th></tr>
            </thead>
            <tbody>
              {#each pod.containers as c (c.role + c.name)}
                <tr>
                  <td class="mono">{c.name}{#if c.role !== 'container'} <span class="tag">{c.role}</span>{/if}</td>
                  <td class:ok={c.ready} class:warn={c.reported && !c.ready && c.role === 'container'}>
                    {c.reported ? stateLine(c.state) : 'not reported yet'}
                    {#if c.state?.since}<span class="dim" title={c.state.since}> · {ago(c.state.since)} ago</span>{/if}
                    {#if c.state?.message}<div class="why">{c.state.message}</div>{/if}
                  </td>
                  <td>
                    <span class:ok={c.ready} class:warn={c.reported && !c.ready && c.role === 'container'}>{c.reported ? (c.ready ? '✓ ready' : '· not ready') : '—'}</span>
                    {#if c.started !== null && c.started !== undefined}<div class="dim">{c.started ? 'started' : 'not started'}</div>{/if}
                  </td>
                  <td class:warn={c.restartCount > 0}>{c.restartCount}</td>
                  <td>
                    {#if c.lastState}
                      {stateLine(c.lastState)}{#if c.lastState.finishedAt}<span class="dim"> · {ago(c.lastState.finishedAt)} ago</span>{/if}
                    {:else if c.restartCount > 0}
                      <span class="dim">not reported ({gapFor('last termination')?.issue || 'rustkube-node#130'}){#if c.runs?.length}{' '}— <a href={`#/pod/${ns}/${name}?tab=Logs&container=${encodeURIComponent(c.name)}`} onclick={() => (tab = 'Logs')}>{c.runs.length} kept {c.runs.length === 1 ? 'run' : 'runs'}</a>{/if}</span>
                    {:else}—{/if}
                  </td>
                  <td class="mono">{resLine(c.resources?.requests) || '—'}</td>
                  <td class="mono">{resLine(c.resources?.limits) || '—'}</td>
                  <td class="mono img">{c.image}</td>
                  <td class="mono">{#each c.ports as p}<div>{p.containerPort}/{p.protocol}{#if p.name} ({p.name}){/if}{#if p.hostPort} → host {p.hostPort}{/if}</div>{:else}—{/each}</td>
                  <td class="logs">
                    <button class="link" onclick={() => openLogs(c.name)}>Logs</button>
                    {#if c.restartCount > 0}<button class="link" onclick={() => openLogs(c.name, 'previous')}>Previous</button>{/if}
                  </td>
                </tr>
              {/each}
            </tbody>
          </table>
        </div>
      </section>

      <section class="card wide" aria-label="Each container">
        <h2>Each container</h2>
        {#each pod.containers as c (c.role + c.name)}
          <details class="ctr" open={pod.containers.length <= 3}>
            <summary><span class="mono">{c.name}</span>{#if c.role !== 'container'} <span class="tag">{c.role}</span>{/if} <span class="dim">{c.reported ? stateLine(c.state) : 'not reported yet'}</span></summary>
            <dl>
              <dt>Image</dt>
              <dd class="mono">{c.image}{#if c.digest} <span class="dim" title={c.digest}>@{c.digest.slice(0, 19)}</span>{/if} <a class="nomono" href={registryHref(c)}>in the registry</a></dd>
              <dt>Command</dt><dd class="mono">{c.command?.length ? c.command.join(' ') : 'the image’s own'}</dd>
              <dt>Arguments</dt><dd class="mono">{c.args?.length ? c.args.join(' ') : '—'}</dd>
              <dt>Working directory</dt><dd class="mono">{c.workingDir || 'the image’s own'}</dd>
              {#if c.containerID}<dt>Container ID</dt><dd class="mono">{c.containerID}</dd>{/if}
              {#if c.state?.startedAt}<dt>Started</dt><dd title={c.state.startedAt}>{ago(c.state.startedAt)} ago</dd>{/if}
              {#if c.state?.finishedAt}<dt>Finished</dt><dd title={c.state.finishedAt}>{ago(c.state.finishedAt)} ago</dd>{/if}
              {#if c.state?.exitCode != null}<dt>Exit code</dt><dd class:warn={c.state.exitCode !== 0}>{c.state.exitCode}</dd>{/if}
              {#if c.securityContext && Object.keys(c.securityContext).length}<dt>Security</dt><dd class="mono">{Object.entries(c.securityContext).map(([k, v]) => `${k}=${typeof v === 'object' ? JSON.stringify(v) : v}`).join(', ')}</dd>{/if}
            </dl>
            <h3>Environment</h3>
            {#if c.env.length || c.envFrom.length}
              <table class="env">
                <tbody>
                  {#each c.envFrom as e, i (i)}
                    <tr><td class="mono">{e.prefix ? `${e.prefix}*` : '*'}</td><td>every key of {e.from === 'secret' ? 'Secret' : 'ConfigMap'} {e.ref}{e.from === 'secret' ? ': values not shown' : ''}</td></tr>
                  {/each}
                  {#each c.env as e (e.name)}
                    <tr><td class="mono">{e.name}</td><td class:mono={!e.from} class:dim={e.from}>{envLine(e)}</td></tr>
                  {/each}
                </tbody>
              </table>
            {:else}<p class="none">None set in the spec.</p>{/if}
            <h3>Mounts</h3>
            {#if c.mounts.length}
              <table class="env">
                <tbody>
                  {#each c.mounts as m (m.path)}
                    {@const v = (pod.volumes || []).find((x) => x.name === m.volume)}
                    <tr><td class="mono">{m.path}{#if m.subPath} <span class="dim">({m.subPath})</span>{/if}</td><td>{m.volume}{#if v} <span class="dim">{v.kind}{v.source ? ` ${v.source}` : ''}</span>{/if}{#if m.readOnly} <span class="tag">read-only</span>{/if}</td></tr>
                  {/each}
                </tbody>
              </table>
            {:else}<p class="none">Nothing mounted.</p>{/if}
          </details>
        {/each}
      </section>

      {#if pod.metadata.startTiming}
        {@const t = pod.metadata.startTiming}
        <section class="card wide" aria-label="Start timing">
          <h2>How it started <span class="dim">— the kubelet’s account</span></h2>
          <div class="timing">{#each t.phases as p (p.name)}<div class:total={p.name === 'total'}><span class="dim">{p.name}</span> <span class="mono">{p.value}</span></div>{/each}</div>
          {#if t.counts.length}<p class="dim">{t.counts.map((c) => `${c.name} ${c.value}`).join(' · ')}</p>{/if}
          {#if t.steps.length}
            <details><summary class="dim">Inside those</summary><div class="timing">{#each t.steps as p (p.name)}<div><span class="dim">{p.name}</span> <span class="mono">{p.value}</span></div>{/each}</div></details>
          {/if}
        </section>
      {/if}

      <section class="card wide">
        <h2>Probes</h2>
        {#if !probeRows.length}
          <p class="none">No container declares a startup, liveness or readiness probe.</p>
        {:else}
          <div class="table-wrap">
            <table>
              <thead><tr><th>Container</th><th>Probe</th><th>Check</th><th>Timing</th><th>Last seen</th><th>Last failure</th></tr></thead>
              <tbody>
                {#each probeRows as r (r.c.name + r.kind)}
                  <tr>
                    <td class="mono">{r.c.name}</td>
                    <td>{r.kind}</td>
                    <td class="mono">{r.p.type} {r.p.target}</td>
                    <td class="mono dim">delay={r.p.initialDelaySeconds}s timeout={r.p.timeoutSeconds}s period={r.p.periodSeconds}s #success={r.p.successThreshold} #failure={r.p.failureThreshold}</td>
                    <td class={probeTone(r.p.result)} title={r.p.resultWhy}>{r.p.result || '—'}</td>
                    <td>
                      {#if r.p.lastFailure}
                        <span class="dim" title={r.p.lastFailure.time}>{ago(r.p.lastFailure.time)} ago{#if r.p.lastFailure.count > 1} ×{r.p.lastFailure.count}{/if}</span>
                        <div class="why">{r.p.lastFailure.message}</div>
                      {:else}<span class="dim">none recorded</span>{/if}
                    </td>
                  </tr>
                {/each}
              </tbody>
            </table>
          </div>
          <p class="dim">A failure is the kubelet's Unhealthy event for that container; a pass is its ready and started flags.</p>
        {/if}
      </section>

      <section class="card wide">
        <h2>Volumes</h2>
        {#if !pod.volumes?.length}
          <p class="none">This pod declares no volumes.</p>
        {:else}
          <div class="table-wrap">
            <table>
              <thead><tr><th>Name</th><th>Type</th><th>Source</th><th>Mounted at</th><th>Claim</th></tr></thead>
              <tbody>
                {#each pod.volumes as v (v.name)}
                  <tr>
                    <td class="mono">{v.name}</td>
                    <td>{v.kind}</td>
                    <td>{v.source || '—'}</td>
                    <td class="mono">
                      {#each v.mounts as m}<div>{m.container}: {m.mountPath}{#if m.subPath} (sub {m.subPath}){/if}{#if m.readOnly} <span class="dim">ro</span>{/if}</div>{:else}<span class="dim">not mounted</span>{/each}
                    </td>
                    <td>
                      {#if v.claim && !v.claim.found}<span class="warn">claim {v.claim.name} not found</span>
                      {:else if v.claim}
                        <a href={`#/grid?id=k8s:pvc:${encodeURIComponent(ns)}/${encodeURIComponent(v.claim.name)}`}>{v.claim.name}</a>
                        <span class:ok={v.claim.phase === 'Bound'} class:warn={v.claim.phase !== 'Bound'}>{v.claim.phase || '—'}</span>
                        <div class="dim">
                          {v.claim.capacity || v.claim.requested || '—'}{#if v.claim.storageClass} · {v.claim.storageClass}{/if}{#if v.claim.accessModes?.length} · {v.claim.accessModes.join(', ')}{/if}
                          {#if v.claim.volumeName} · PV {v.claim.volumeName}{#if v.claim.pv?.driver} ({v.claim.pv.driver}){/if}{#if v.claim.pv?.reclaimPolicy}, {v.claim.pv.reclaimPolicy}{/if}{/if}
                        </div>
                      {:else}—{/if}
                    </td>
                  </tr>
                {/each}
              </tbody>
            </table>
          </div>
          <p class="dim">Used space per volume: on the Stats tab, when the node reports it.</p>
        {/if}
      </section>

      <section class="card wide">
        <h2>Images</h2>
        {#each pod.containers as c (c.role + c.name)}
          <div class="image">
            <h3>{c.name}</h3>
            <dl>
              <dt>Reference</dt><dd class="mono">{c.image}<CopyButton value={c.image} label="Copy image" /></dd>
              {#if c.runningImage && c.runningImage !== c.image}<dt>Running</dt><dd class="mono">{c.runningImage}</dd>{/if}
              <dt>Digest</dt>
              <dd class="mono">
                {#if c.digest}<span title={c.digest}>{c.digest}</span><CopyButton value={c.digest} label="Copy digest" />
                {:else if !c.reported}<span class="dim">not reported yet</span>
                {:else}<span class="dim">not reported — the node’s imageID is “{c.imageID || 'empty'}” ({gapFor('image digest')?.issue})</span>{/if}
              </dd>
              {#if c.imageID && c.imageID !== c.digest}<dt>Image ID</dt><dd class="mono">{c.imageID}</dd>{/if}
              <dt>Pull policy</dt><dd>{c.pullPolicy}{#if c.pullPolicyDefaulted} <span class="dim">(defaulted)</span>{/if}</dd>
              <dt>Last checked</dt>
              <dd>
                {#if c.imageResolved}<span title={c.imageResolved}>{ago(c.imageResolved)} ago</span>
                {:else}<span class="dim">not recorded by the node ({gapFor('image last checked')?.issue})</span>{/if}
              </dd>
              {#if c.source === 'stormpump'}
                <!-- What this container's instance was cloned from (#70). The
                     one place the API's word is shown: the JSON field is
                     `golden`, and someone matching the page to it needs to
                     know that. -->
                <dt>
                  Registry image
                  <span class="info" title={`registry image (golden) — “golden” is the API’s name for it${c.golden?.name ? `; stormcentral’s name for this one is ${c.golden.name}` : ''}`}>ⓘ</span>
                </dt>
                <dd>
                  {#if c.golden?.available}
                    <span class="mono">{c.golden.component}@{(c.golden.commit || '').slice(0, 12)}</span>
                    <span class="dim">(sealed digest {c.golden.deviceSha256 ? `sha256:${short(c.golden.deviceSha256)}` : 'not recorded'}, built {builtAt(c.golden).slice(0, 10)}{sourcesLine(c.golden)})</span>
                    <div class="dim">{c.golden.version || ''} · build {c.golden.buildId || '—'} · by {c.golden.builtBy || '—'}</div>
                    <div class="dim">{c.golden.which}</div>
                  {:else}
                    <span class="dim">{c.golden?.reason || 'unknown'}</span>
                  {/if}
                </dd>
              {/if}
              <dt>Built</dt>
              <dd>
                {#if c.source === 'stormpump'}
                  {#if c.golden?.available}{builtAt(c.golden)}{:else}<span class="dim">—</span>{/if}
                {:else}
                  <span class="dim">{gapFor('build info')?.why} ({gapFor('build info')?.issue})</span>
                {/if}
              </dd>
            </dl>
          </div>
        {/each}
      </section>

      <section class="cards">
        <div class="card">
          <h2>Labels</h2>
          {#if kv(pod.metadata.labels).length}
            <ul class="kv">{#each kv(pod.metadata.labels) as [k, v] (k)}<li><span class="k">{k}</span>={v}</li>{/each}</ul>
          {:else}<p class="none">No labels.</p>{/if}
        </div>
        <div class="card">
          <h2>Annotations</h2>
          {#if kv(pod.metadata.annotations).length}
            <ul class="kv">{#each kv(pod.metadata.annotations) as [k, v] (k)}<li title={v}><span class="k">{k}</span>={v}</li>{/each}</ul>
          {:else}<p class="none">No annotations.</p>{/if}
        </div>
        <div class="card">
          <h2>Not reported by the node</h2>
          <ul class="gaps">
            {#each pod.gaps as g (g.what)}<li><strong>{g.what}</strong> — {g.why} <span class="mono">({g.issue})</span></li>{/each}
          </ul>
        </div>
      </section>

      <section class="card events-card">
        <EventBox id={`k8s:pod:${ns}/${name}`} />
      </section>
    {:else if tab === 'Network'}
      <section class="cards">
        <div class="card">
          <h2>Addresses</h2>
          <dl>
            <dt>Pod IPs</dt>
            <dd class="mono">{#each pod.network.podIPs as ip (ip)}<div>{ip}<CopyButton value={ip} label="Copy address" /></div>{:else}—{/each}</dd>
            <dt>Host network</dt><dd>{pod.network.hostNetwork ? 'yes — the node’s interfaces and addresses' : 'no'}</dd>
            <dt>Node address</dt><dd class="mono">{pod.network.hostIP || '—'}</dd>
          </dl>
        </div>
        <div class="card">
          <h2>DNS</h2>
          <dl>
            <dt>Policy</dt><dd>{pod.network.dns.policy}{#if pod.network.dns.effective !== pod.network.dns.policy} <span class="dim">→ {pod.network.dns.effective}</span>{/if}</dd>
            {#if pod.network.dns.nameservers.length}<dt>Nameservers</dt><dd class="mono">{pod.network.dns.nameservers.join(', ')}</dd>{/if}
            {#if pod.network.dns.searches.length}<dt>Search</dt><dd class="mono">{pod.network.dns.searches.join(' ')}</dd>{/if}
            {#if pod.network.dns.options.length}<dt>Options</dt><dd class="mono">{pod.network.dns.options.map((o) => (o.value ? `${o.name}:${o.value}` : o.name)).join(' ')}</dd>{/if}
            {#if pod.network.dns.hostname}<dt>Hostname</dt><dd class="mono">{pod.network.dns.hostname}{pod.network.dns.subdomain ? `.${pod.network.dns.subdomain}` : ''}</dd>{/if}
          </dl>
          {#if !pod.network.dns.nameservers.length}<p class="dim">The resolver the policy gives is written by the node into the pod; it is not reported back.</p>{/if}
        </div>
        <div class="card">
          <h2>Cilium</h2>
          {#if !pod.network.cilium}
            <p class="none">{pod.network.hostNetwork ? 'Host network: Cilium does not manage this pod’s interface.' : 'No Cilium endpoint for this pod.'}</p>
          {:else}
            <dl>
              <dt>Datapath</dt><dd class:ok={pod.network.cilium.state === 'ready'} class:warn={pod.network.cilium.state !== 'ready'}>{pod.network.cilium.state || '—'}</dd>
              <dt>Identity</dt><dd class="mono">{pod.network.cilium.identity ?? '—'}{#if pod.network.cilium.identityLabels} · {pod.network.cilium.identityLabels}{/if}</dd>
              <dt>Policies</dt>
              <dd>
                {#each pod.network.cilium.policies as p (p)}<div class="mono"><a href={`#/grid?id=${encodeURIComponent(p)}`}>{p.replace(/^k8s:/, '')}</a></div>{:else}<span class="dim">none select it</span>{/each}
              </dd>
            </dl>
          {/if}
        </div>
      </section>

      {#if pod.network.attachments?.length}
        <section class="card wide" aria-label="Attachments">
          <h2>Networks it asked for</h2>
          <table>
            <thead><tr><th>Network</th><th>Project</th><th>Type</th><th>Interface</th><th>Addresses</th><th>Attached</th></tr></thead>
            <tbody>
              {#each pod.network.attachments as a (a.namespace + a.name)}
                <tr>
                  <td>{#if a.id}<a href={`#/grid?id=${encodeURIComponent(a.id)}`}>{a.name}</a>{:else}{a.name}{/if}{#if !a.exists} <span class="warn">no such network</span>{/if}</td>
                  <td>{#if a.inProject}<span class="dim">this one</span>{:else}<a href={`#/k8s/ns/${encodeURIComponent(a.namespace)}`}>{a.namespace}</a>{/if}</td>
                  <td>{a.type || '—'}</td>
                  <td class="mono">{a.interface || '—'}</td>
                  <td class="mono">{(a.ips || []).join(', ') || '—'}</td>
                  <td class:ok={a.attached} class:warn={!a.attached}>{a.attached ? 'yes' : 'not reported'}</td>
                </tr>
              {/each}
            </tbody>
          </table>
        </section>
      {/if}

      <section class="card wide">
        <h2>Interfaces</h2>
        {#if pod.network.interfaces.length}
          <table>
            <thead><tr><th>Name</th><th>Interface</th><th>Addresses</th><th>MAC</th><th>MTU</th><th>Gateway</th><th>Default</th></tr></thead>
            <tbody>
              {#each pod.network.interfaces as i, n (n)}
                <tr>
                  <td>{i.name || '—'}</td><td class="mono">{i.interface || '—'}</td>
                  <td class="mono">{(i.ips || []).join(', ') || '—'}</td><td class="mono">{i.mac || '—'}</td>
                  <td>{i.mtu ?? '—'}</td><td class="mono">{(i.gateway || []).join?.(', ') || i.gateway || '—'}</td><td>{i.default ? 'yes' : ''}</td>
                </tr>
              {/each}
            </tbody>
          </table>
        {:else}
          <p class="none">{gapFor('interface detail')?.why || 'Not reported.'} ({gapFor('interface detail')?.issue})</p>
        {/if}

        <h2 class="second">Traffic</h2>
        {#if !traffic}
          <p class="none">Reading the node…</p>
        {:else if !traffic.available}
          <p class="none">{traffic.reason}</p>
        {:else}
          <table class="traffic">
            <thead><tr><th>Interface</th><th>Received</th><th>Sent</th><th>Receive rate</th><th>Send rate</th><th>Errors</th><th>Drops</th></tr></thead>
            <tbody>
              {#each traffic.interfaces as i (i.interface)}
                {@const h = history[i.interface] || []}
                <tr>
                  <td class="mono">{i.interface}</td>
                  <td class="num">{formatBytes(i.rxBytes ?? 0)}{#if i.rxPackets !== undefined}<div class="dim">{i.rxPackets} pkts</div>{/if}</td>
                  <td class="num">{formatBytes(i.txBytes ?? 0)}{#if i.txPackets !== undefined}<div class="dim">{i.txPackets} pkts</div>{/if}</td>
                  <td>
                    <svg class="spark" viewBox="0 0 160 28" preserveAspectRatio="none" role="img" aria-label="receive rate"><polyline points={spark(rates(h, 'rx'))} /></svg>
                    <div class="num">{perSec(rate(h, 'rx'))}</div>
                  </td>
                  <td>
                    <svg class="spark tx" viewBox="0 0 160 28" preserveAspectRatio="none" role="img" aria-label="send rate"><polyline points={spark(rates(h, 'tx'))} /></svg>
                    <div class="num">{perSec(rate(h, 'tx'))}</div>
                  </td>
                  <td class="num">{i.rxErrors !== undefined ? `${i.rxErrors} / ${i.txErrors ?? 0}` : '—'}</td>
                  <td class="num">{i.rxDropped !== undefined ? `${i.rxDropped} / ${i.txDropped ?? 0}` : '—'}</td>
                </tr>
              {/each}
            </tbody>
          </table>
          <p class="dim">From the kubelet on {traffic.node}, read every 5 s while this page is open; rates are between reads.{#if traffic.missing}{' '}Not exported: {traffic.missing}.{/if}</p>
        {/if}
      </section>

      <section class="card wide">
        <h2>Services that select this pod</h2>
        {#if !pod.network.services.length}
          <p class="none">No Service in {ns} selects this pod’s labels.</p>
        {:else}
          <table>
            <thead><tr><th>Service</th><th>Type</th><th>Cluster IP</th><th>Ports</th><th>Endpoints</th><th>This pod</th></tr></thead>
            <tbody>
              {#each pod.network.services as s (s.id)}
                <tr>
                  <td><a href={`#/grid?id=${encodeURIComponent(s.id)}`}>{s.name}</a></td>
                  <td>{s.type}</td>
                  <td class="mono">{s.clusterIPs.join(', ') || '—'}</td>
                  <td class="mono">{#each s.ports as p}<div>{p.port}{#if p.targetPort !== undefined && p.targetPort !== p.port}→{p.targetPort}{/if}/{p.protocol || 'TCP'}{#if p.nodePort} (node {p.nodePort}){/if}</div>{/each}</td>
                  {#if s.endpoints?.error}
                    <td colspan="2" class="dim">{s.endpoints.error}</td>
                  {:else}
                    <td>{s.endpoints?.ready ?? '—'} ready{#if s.endpoints?.notReady}, {s.endpoints.notReady} not{/if}</td>
                    <td class:ok={s.endpoints?.thisPod === 'ready'} class:warn={s.endpoints?.thisPod !== 'ready'}>
                      {s.endpoints?.thisPod === 'ready' ? 'receiving traffic' : s.endpoints?.thisPod === 'notReady' ? 'not ready — no traffic' : 'not in the endpoints'}
                    </td>
                  {/if}
                </tr>
              {/each}
            </tbody>
          </table>
        {/if}
        <h2 class="second">Container ports</h2>
        <p class="mono">
          {#each pod.containers.filter((c) => c.ports.length) as c}
            <span class="port">{c.name}: {c.ports.map((p) => `${p.containerPort}/${p.protocol}`).join(', ')}</span>
          {:else}<span class="dim">No container declares a port.</span>{/each}
        </p>
      </section>
    {:else if tab === 'Stats'}
      <PodStats {base} containers={pod.containers} />
    {:else if tab === 'Logs'}
      {#key logContainer + logSource}
        <PodLogs {ns} {name} containers={pod.containers} keep={pod.keptRuns} initial={logContainer} initialSource={logSource} />
      {/key}
    {:else if tab === 'Events'}
      <section class="card">
        <h2>Events <span class="dim">— followed every 5 s</span></h2>
        {#if !events}
          <p class="none">Reading…</p>
        {:else if !events.available}
          <p class="none">{events.reason}</p>
        {:else if !events.items.length}
          <p class="none">Nothing has been recorded about this pod.</p>
        {:else}
          <div class="table-wrap">
            <table class="events">
              <thead><tr><th>Type</th><th>Reason</th><th>Container</th><th>Count</th><th>First seen</th><th>Last seen</th><th>From</th><th>Message</th></tr></thead>
              <tbody>
                {#each events.items as e, i (i)}
                  <tr class:warnrow={e.type === 'Warning'}>
                    <td>{e.type}</td>
                    <td><strong>{e.reason}</strong></td>
                    <td class="mono">{e.container || '—'}</td>
                    <td class="num">{e.count}</td>
                    <td class="dim" title={e.first}>{e.first ? `${ago(e.first)} ago` : '—'}</td>
                    <td class="dim" title={e.time}>{e.time ? `${ago(e.time)} ago` : '—'}</td>
                    <td class="dim">{e.source || '—'}</td>
                    <td>{e.message}</td>
                  </tr>
                {/each}
              </tbody>
            </table>
          </div>
        {/if}
      </section>
    {:else}
      <YamlPanel {yaml} label="Pod {name} in {ns}" savePath={`/api/plugins/k8s/object/pod/${encodeURIComponent(ns)}/${encodeURIComponent(name)}`} />
    {/if}
  {/if}
</div>

<style>
  .ctr { border-top: 1px solid var(--sc-hairline); padding: 8px 0; }
  .ctr summary { cursor: pointer; }
  .ctr h3 { font-size: var(--sc-t-meta); color: var(--text-faint); margin: 10px 0 4px; }
  .ctr dl { margin-top: 8px; }
  table.env td:first-child { white-space: nowrap; padding-right: 16px; width: 1%; }
  table.env td { overflow-wrap: anywhere; }
  .nomono { font-family: var(--font, inherit); margin-left: 8px; }
  .timing { display: flex; gap: 6px 18px; flex-wrap: wrap; }
  .timing .total { font-weight: 600; }
  td.logs button + button { margin-left: 8px; }
  .tabs { display: flex; gap: 2px; border-bottom: 1px solid var(--border); margin-bottom: 14px; }
  .tabs button {
    background: none; border: none; border-bottom: 2px solid transparent; border-radius: 0;
    padding: 7px 14px; font-size: var(--sc-t-body); color: var(--text-dim);
  }
  .tabs button:hover { color: var(--text); background: var(--nav-hover); }
  .tabs button.active { color: var(--text); border-bottom-color: var(--accent); font-weight: 600; }

  .cards { display: grid; grid-template-columns: repeat(auto-fit, minmax(300px, 1fr)); gap: 12px; align-items: start; margin-bottom: 12px; }
  .card { background: var(--panel); border: 1px solid var(--border); border-radius: var(--radius); padding: 14px var(--sc-row-px); }
  .card.wide { margin-bottom: 12px; }
  h2 { font-size: var(--sc-t-eyebrow); text-transform: uppercase; letter-spacing: 0.06em; color: var(--text-faint); margin: 0 0 10px; }
  h2.second { margin-top: 18px; }
  h3 { font-size: var(--sc-t-body); margin: 0 0 6px; font-weight: 600; }
  dl { display: grid; grid-template-columns: max-content 1fr; gap: 4px 14px; margin: 0; font-size: var(--sc-t-body); }
  dt { color: var(--text-dim); }
  dd { margin: 0; min-width: 0; overflow-wrap: anywhere; }
  .mono { font-family: var(--mono); font-size: var(--sc-t-meta); }
  .dim { color: var(--text-faint); font-size: var(--sc-t-meta); }
  .none { margin: 0; color: var(--text-dim); font-size: var(--sc-t-body); }
  .reason, .stale { font-size: var(--sc-t-meta); color: var(--warn, #d29922); margin: 8px 0 0; }
  .ok { color: var(--ok, #3fb950); }
  .warn { color: var(--warn, #d29922); }
  .why { font-size: var(--sc-t-meta); color: var(--text-dim); }
  tr.why td { border-top: none; padding-top: 0; }
  .tag { font-size: var(--sc-t-eyebrow); border: 1px solid var(--border); border-radius: 999px; padding: 0 6px; color: var(--text-faint); }
  .table-wrap { overflow-x: auto; }
  table { width: 100%; border-collapse: collapse; font-size: var(--sc-t-body); }
  th { text-align: left; font-size: var(--sc-t-eyebrow); text-transform: uppercase; letter-spacing: 0.05em; color: var(--text-faint); font-weight: 600; padding: 3px 8px 3px 0; }
  td { padding: 4px 8px 4px 0; border-top: 1px solid var(--sc-hairline); vertical-align: top; }
  td.img { max-width: 320px; overflow-wrap: anywhere; }
  .num { font-variant-numeric: tabular-nums; }
  .image + .image { margin-top: 14px; padding-top: 12px; border-top: 1px solid var(--sc-hairline); }
  .kv, .gaps { list-style: none; margin: 0; padding: 0; display: grid; gap: 3px; font-size: var(--sc-t-meta); }
  .kv li { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; color: var(--text-dim); }
  .kv .k { color: var(--text); font-family: var(--mono); }
  .gaps li { color: var(--text-dim); }
  .iface { display: flex; gap: 14px; font-size: var(--sc-t-body); font-variant-numeric: tabular-nums; }
  .spark { width: 160px; height: 28px; display: block; }
  .spark polyline { fill: none; stroke: var(--accent); stroke-width: 1.5; }
  .spark.tx polyline { stroke: var(--ok, #3fb950); }
  .port { margin-right: 14px; }
  button.link { background: none; border: none; color: var(--accent); padding: 0; cursor: pointer; }
  button.danger { color: var(--error, #f85149); }
  .events-card { margin-top: 12px; }
  tr.warnrow td { background: var(--warn-bg); }
  tr.warnrow td:first-child { color: var(--warn-strong); font-weight: 600; }
  .info { cursor: help; color: var(--text-faint); font-size: var(--sc-t-meta); }
</style>
