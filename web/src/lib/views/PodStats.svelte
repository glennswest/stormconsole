<script>
  // A pod's stats over time (#124): `kubectl top pod --containers` and the
  // rest of the kubelet's Summary API, as charts over 15 minutes or an
  // hour, from what the console scrapes of the node's kubelet every 15 s.
  //
  // What the node does not report is named with the issue that would
  // close it (rustkube-node#242), not drawn as zero.
  import { onDestroy } from 'svelte'
  import { get, formatBytes } from '../api.js'
  import LineChart from '../components/LineChart.svelte'

  let { base, containers = [] } = $props()

  let windowSecs = $state(900)
  let data = $state(null)
  let error = $state('')
  let timer = null

  async function load() {
    try {
      data = await get(`${base}/stats?window=${windowSecs}`)
      error = ''
    } catch (e) {
      error = e.message
    }
    clearTimeout(timer)
    timer = setTimeout(load, 15000)
  }
  $effect(() => {
    void windowSecs
    load()
  })
  onDestroy(() => clearTimeout(timer))

  const to = $derived(data?.now ?? Date.now())
  const from = $derived(to - windowSecs * 1000)
  // The colour slot is the container's place in the pod, so it keeps its
  // colour whatever else is reported.
  const order = $derived(containers.filter((c) => c.role !== 'ephemeral').map((c) => c.name))
  const slotOf = (name) => {
    const i = order.indexOf(name)
    return i < 0 ? order.length : i
  }
  const ctrs = $derived(data?.series?.containers || [])
  const ifaces = $derived(data?.series?.interfaces || [])
  const line = (list, key, name = (c) => c.name, slot = (c) => slotOf(c.name)) =>
    list.map((c) => ({ name: name(c), slot: slot(c), points: c.points.map((p) => ({ t: p.t, v: p[key] })) }))
  const has = (list, key) => list.some((c) => c.points.some((p) => p[key] != null))

  const cores = (v) => (v >= 1 ? `${v.toFixed(2)} cores` : `${Math.round(v * 1000)}m`)
  // stormview's formatBytes has no unit below one byte (a slow rate is).
  const bytes = (v) => (v < 1 ? `${Math.round(v * 10) / 10} B` : formatBytes(v))
  const bps = (v) => `${bytes(v)}/s`
  const pps = (v) => `${v >= 100 ? Math.round(v) : v.toFixed(1)} pkt/s`
  const count = (v) => String(Math.round(v))
  const n = (v) => (v == null ? '—' : String(v))
  const fsLine = (f) => (f ? `${formatBytes(f.usedBytes ?? 0)}${f.capacityBytes ? ` of ${formatBytes(f.capacityBytes)}` : ''}` : '—')
  const since = $derived(data?.since ? new Date(data.since).toLocaleTimeString() : '')
</script>

<div class="stats">
  <div class="bar">
    <label>Window
      <select bind:value={windowSecs} aria-label="Window">
        <option value={900}>15 minutes</option>
        <option value={3600}>1 hour</option>
      </select>
    </label>
    {#if data?.available}
      <span class="dim">every {data.every} s from the kubelet on {data.node} · {data.samples} samples kept{#if since} · sampling since {since}{/if}</span>
    {/if}
  </div>

  {#if error}<p class="error">{error}</p>{/if}

  {#if data && !data.available}
    <p class="none">{data.reason}</p>
  {:else if data}
    <section class="grid">
      <div class="card">
        <h2>CPU</h2>
        {#if has(ctrs, 'cpu')}
          <LineChart label="CPU by container" series={line(ctrs, 'cpu')} {from} {to} format={cores} />
        {:else}<p class="none">Not enough samples for a rate yet (two are needed, 15 s apart).</p>{/if}
      </div>
      <div class="card">
        <h2>Memory — working set</h2>
        {#if has(ctrs, 'workingSet')}
          <LineChart label="Memory working set by container" series={line(ctrs, 'workingSet')} {from} {to} format={bytes} />
        {:else}<p class="none">The node reports no memory for these containers.</p>{/if}
      </div>
      {#if has(ctrs, 'rss')}
        <div class="card">
          <h2>Memory — RSS</h2>
          <LineChart label="RSS by container" series={line(ctrs, 'rss')} {from} {to} format={bytes} />
        </div>
      {/if}
      {#if has(ctrs, 'pageFaults')}
        <div class="card">
          <h2>Page faults (cumulative)</h2>
          <LineChart label="Page faults by container" series={line(ctrs, 'pageFaults')} {from} {to} format={count} />
        </div>
      {/if}
      {#each ifaces as i (i.name)}
        <div class="card">
          <h2>Network {i.name} — bytes</h2>
          <LineChart
            label="{i.name} bytes per second"
            series={[
              { name: 'received', slot: 0, points: i.points.map((p) => ({ t: p.t, v: p.rxBps })) },
              { name: 'sent', slot: 1, points: i.points.map((p) => ({ t: p.t, v: p.txBps })) },
            ]}
            {from}
            {to}
            format={bps}
          />
        </div>
        {#if i.points.some((p) => p.rxPps != null)}
          <div class="card">
            <h2>Network {i.name} — packets</h2>
            <LineChart
              label="{i.name} packets per second"
              series={[
                { name: 'received', slot: 0, points: i.points.map((p) => ({ t: p.t, v: p.rxPps })) },
                { name: 'sent', slot: 1, points: i.points.map((p) => ({ t: p.t, v: p.txPps })) },
              ]}
              {from}
              {to}
              format={pps}
            />
          </div>
        {/if}
      {/each}
    </section>

    {#if ifaces.length}
      <section class="card">
        <h2>Interfaces — totals since the sandbox started</h2>
        <table>
          <thead><tr><th>Interface</th><th>Received</th><th>Sent</th><th>Packets in / out</th><th>Errors in / out</th><th>Dropped in / out</th></tr></thead>
          <tbody>
            {#each ifaces as i (i.name)}
              {@const t = i.totals}
              <tr>
                <td class="mono">{i.name}</td>
                <td class="num">{t.rxBytes == null ? '—' : formatBytes(t.rxBytes)}</td>
                <td class="num">{t.txBytes == null ? '—' : formatBytes(t.txBytes)}</td>
                <td class="num">{n(t.rxPackets)} / {n(t.txPackets)}</td>
                <td class="num" class:warn={(t.rxErrors || 0) + (t.txErrors || 0) > 0}>{n(t.rxErrors)} / {n(t.txErrors)}</td>
                <td class="num" class:warn={(t.rxDropped || 0) + (t.txDropped || 0) > 0}>{n(t.rxDropped)} / {n(t.txDropped)}</td>
              </tr>
            {/each}
          </tbody>
        </table>
      </section>
    {/if}

    <section class="card">
      <h2>Storage</h2>
      <table>
        <tbody>
          <tr><th>Ephemeral storage</th><td>{fsLine(data.latest?.ephemeralStorage)}</td></tr>
          {#each data.latest?.containers || [] as c (c.name)}
            <tr><th>{c.name}</th><td>root filesystem {fsLine(c.rootfs)} · logs {fsLine(c.logs)}</td></tr>
          {/each}
          {#each data.latest?.volume || [] as v (v.name)}
            <tr><th>volume {v.name}{#if v.pvcRef} <span class="dim">(claim {v.pvcRef.name})</span>{/if}</th><td>{fsLine(v)}</td></tr>
          {/each}
        </tbody>
      </table>
    </section>

    {#if data.missing?.length}
      <section class="card">
        <h2>Not reported by the node</h2>
        <ul class="gaps">{#each data.missing as m (m)}<li>{m}</li>{/each}</ul>
      </section>
    {/if}
  {:else}
    <p class="none">Reading…</p>
  {/if}
</div>

<style>
  .stats { display: grid; gap: 12px; }
  .bar { display: flex; flex-wrap: wrap; gap: 12px; align-items: center; font-size: var(--sc-t-meta); }
  .bar label { display: inline-flex; gap: 6px; align-items: center; }
  .grid { display: grid; grid-template-columns: repeat(auto-fit, minmax(420px, 1fr)); gap: 12px; }
  .card { background: var(--panel); border: 1px solid var(--border); border-radius: var(--radius); padding: 12px var(--sc-row-px); min-width: 0; }
  h2 { font-size: var(--sc-t-eyebrow); text-transform: uppercase; letter-spacing: 0.06em; color: var(--text-faint); margin: 0 0 8px; }
  table { border-collapse: collapse; width: 100%; font-size: var(--sc-t-body); }
  th { text-align: left; font-weight: 500; color: var(--text-dim); padding: 4px 12px 4px 0; font-size: var(--sc-t-meta); }
  td { padding: 4px 12px 4px 0; }
  .num { font-variant-numeric: tabular-nums; font-family: var(--mono); font-size: var(--sc-t-meta); }
  .mono { font-family: var(--mono); font-size: var(--sc-t-meta); }
  .warn { color: var(--warn-strong); }
  .dim { color: var(--text-dim); }
  .none { margin: 0; color: var(--text-faint); font-size: var(--sc-t-meta); }
  .error { color: var(--error); }
  .gaps { margin: 0; padding-left: 18px; font-size: var(--sc-t-meta); color: var(--text-dim); }
  @media (max-width: 600px) { .grid { grid-template-columns: 1fr; } }
</style>
