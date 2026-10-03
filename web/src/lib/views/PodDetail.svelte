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

  const ns = $derived(route.current.params.ns)
  const name = $derived(route.current.params.name)
  const TABS = ['Overview', 'Network', 'Logs', 'Events', 'YAML']
  let tab = $state(TABS.includes(route.current.query.get('tab')) ? route.current.query.get('tab') : 'Overview')
  const logContainer = route.current.query.get('container') || ''

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
  const builtAt = (g) => (g?.builtAt ? new Date(g.builtAt * 1000).toISOString().replace('.000', '') : '')
  const gapFor = (what) => pod?.gaps?.find((g) => g.what === what)
  const invoke = (a) => call(a.method, a.path)
  const deleteAction = $derived(component?.actions?.find((a) => a.id === 'delete'))
</script>

<div class="sc-page">
  <PageHeader
    crumbs={[{ label: 'Workloads' }, { label: 'Pods', href: `#/k8s/pod?ns=${encodeURIComponent(ns)}` }, { label: ns, href: `#/k8s/ns/${encodeURIComponent(ns)}` }, { label: name }]}
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
            <dt>Started</dt><dd>{pod.metadata.startTime ? `${ago(pod.metadata.startTime)} ago` : '—'}</dd>
            <dt>QoS</dt><dd>{pod.metadata.qosClass}</dd>
            <dt>Priority</dt><dd>{pod.metadata.priority ?? '—'}{#if pod.metadata.priorityClassName} · {pod.metadata.priorityClassName}{/if}</dd>
            <dt>Service account</dt><dd class="mono">{pod.metadata.serviceAccount || '—'}</dd>
            <dt>Restart policy</dt><dd>{pod.metadata.restartPolicy}</dd>
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
              <tr><th>Name</th><th>State</th><th>Restarts</th><th>Last termination</th><th>Image</th><th>Ports</th><th></th></tr>
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
                  <td class:warn={c.restartCount > 0}>{c.restartCount}</td>
                  <td>
                    {#if c.lastState}
                      {stateLine(c.lastState)}{#if c.lastState.finishedAt}<span class="dim"> · {ago(c.lastState.finishedAt)} ago</span>{/if}
                    {:else if c.restartCount > 0}
                      <span class="dim">not reported ({gapFor('last termination')?.issue || 'rustkube-node#130'}){#if c.runs?.length}{' '}— <a href={`#/pod/${ns}/${name}?tab=Logs&container=${encodeURIComponent(c.name)}`} onclick={() => (tab = 'Logs')}>{c.runs.length} kept {c.runs.length === 1 ? 'run' : 'runs'}</a>{/if}</span>
                    {:else}—{/if}
                  </td>
                  <td class="mono img">{c.image}</td>
                  <td class="mono">{#each c.ports as p}<div>{p.containerPort}/{p.protocol}{#if p.name} ({p.name}){/if}{#if p.hostPort} → host {p.hostPort}{/if}</div>{:else}—{/each}</td>
                  <td><button class="link" onclick={() => (tab = 'Logs')}>Logs</button></td>
                </tr>
              {/each}
            </tbody>
          </table>
        </div>
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
              <dt>Built</dt>
              <dd>
                {#if c.source === 'stormpump'}
                  {#if c.golden?.available}
                    {builtAt(c.golden)} by {c.golden.builtBy || '—'}
                    <div class="dim">{c.golden.name} · {c.golden.version || ''} · commit <span class="mono">{(c.golden.commit || '').slice(0, 12)}</span> · build {c.golden.buildId || '—'}</div>
                    {#if c.golden.tarSha256}<div class="dim mono">tar sha256:{short(c.golden.tarSha256)}</div>{/if}
                    <div class="dim">{c.golden.which}</div>
                  {:else}
                    <span class="dim">{c.golden?.reason || 'unknown'}</span>
                  {/if}
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
    {:else if tab === 'Logs'}
      <PodLogs {ns} {name} containers={pod.containers} keep={pod.keptRuns} initial={logContainer} />
    {:else if tab === 'Events'}
      <section class="card events-card">
        <EventBox id={`k8s:pod:${ns}/${name}`} />
      </section>
    {:else}
      <YamlPanel {yaml} label="Pod {name} in {ns}" savePath={`/api/plugins/k8s/object/pod/${encodeURIComponent(ns)}/${encodeURIComponent(name)}`} />
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
</style>
