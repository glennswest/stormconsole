<script>
  // A small line chart for a measure over time (#124): one y-axis, thin
  // lines, a crosshair and tooltip on hover, a legend when there is more
  // than one series, and the numbers in a table for anyone who would rather
  // read them.
  //
  // Colour follows the series' `slot` (its place in a fixed order — the
  // container's place in the pod, rx before tx), never its rank, so a
  // series keeps its colour when another drops out. The slots are the
  // dataviz reference palette's first eight, each stepped for a light and
  // a dark surface; which one applies is read off the console's own
  // background, because the console has twelve palettes, not two.
  //
  // A null value is a gap, not a zero: the node did not report it.
  let { series = [], from, to, format = (v) => String(v), height = 140, label = '' } = $props()

  const LIGHT = ['#2a78d6', '#eb6834', '#1baf7a', '#eda100', '#e87ba4', '#008300', '#6250d6', '#e34948']
  const DARK = ['#3987e5', '#d95926', '#199e70', '#c98500', '#d55181', '#008300', '#9085e9', '#e66767']

  function darkSurface() {
    try {
      const bg = getComputedStyle(document.documentElement).getPropertyValue('--bg').trim()
      const c = document.createElement('canvas').getContext('2d')
      c.fillStyle = bg || '#fff'
      const hex = c.fillStyle
      if (!hex.startsWith('#')) return false
      const n = parseInt(hex.slice(1), 16)
      const l = 0.2126 * ((n >> 16) & 255) + 0.7152 * ((n >> 8) & 255) + 0.0722 * (n & 255)
      return l < 128
    } catch {
      return false
    }
  }
  const palette = darkSurface() ? DARK : LIGHT
  const color = (s) => palette[(s.slot ?? 0) % palette.length]

  let width = $state(600)
  const PAD = { l: 56, r: 12, t: 8, b: 20 }
  const plotW = $derived(Math.max(10, width - PAD.l - PAD.r))
  const plotH = $derived(height - PAD.t - PAD.b)

  const values = $derived(series.flatMap((s) => s.points.map((p) => p.v)).filter((v) => v !== null && v !== undefined))
  const max = $derived(niceMax(values.length ? Math.max(...values) : 0))
  function niceMax(v) {
    if (v <= 0) return 1
    const e = Math.pow(10, Math.floor(Math.log10(v)))
    for (const m of [1, 2, 2.5, 5, 10]) if (m * e >= v) return m * e
    return 10 * e
  }
  const x = (t) => PAD.l + ((t - from) / Math.max(1, to - from)) * plotW
  const y = (v) => PAD.t + plotH - (v / max) * plotH

  // Lines broken at nulls.
  function paths(s) {
    const out = []
    let cur = ''
    for (const p of s.points) {
      if (p.v === null || p.v === undefined || p.t < from) {
        if (cur) out.push(cur)
        cur = ''
        continue
      }
      cur += `${cur ? 'L' : 'M'}${x(p.t).toFixed(1)},${y(p.v).toFixed(1)}`
    }
    if (cur) out.push(cur)
    return out
  }
  // A lone point between gaps would draw nothing: mark it.
  function singles(s) {
    const pts = s.points.filter((p) => p.t >= from)
    return pts.filter((p, i) => p.v != null && (pts[i - 1]?.v == null) && (pts[i + 1]?.v == null))
  }

  const ticks = $derived([0, 0.5, 1].map((f) => f * max))
  // The tooltip's format, to the minute: one clock on the whole chart.
  const clock = (t) => new Date(t).toLocaleTimeString([], { hour: 'numeric', minute: '2-digit' })
  const xticks = $derived([from, from + (to - from) / 2, to])

  let hover = $state(null)
  const times = $derived([...new Set(series.flatMap((s) => s.points.map((p) => p.t)))].filter((t) => t >= from).sort((a, b) => a - b))
  function move(ev) {
    const r = ev.currentTarget.getBoundingClientRect()
    const px = ev.clientX - r.left
    if (!times.length) return
    const t = from + ((px - PAD.l) / plotW) * (to - from)
    let best = times[0]
    for (const c of times) if (Math.abs(c - t) < Math.abs(best - t)) best = c
    hover = { t: best, rows: series.map((s) => ({ s, v: s.points.find((p) => p.t === best)?.v ?? null })) }
  }
  const last = (s) => {
    for (let i = s.points.length - 1; i >= 0; i--) if (s.points[i].v != null) return s.points[i].v
    return null
  }
</script>

<figure class="chart">
  {#if series.length > 1}
    <ul class="legend" aria-label="{label} legend">
      {#each series as s (s.name)}
        <li><span class="key" style="background:{color(s)}"></span>{s.name} <span class="now">{last(s) === null ? '—' : format(last(s))}</span></li>
      {/each}
    </ul>
  {:else if series.length === 1}
    <p class="single">now <span class="now">{last(series[0]) === null ? '—' : format(last(series[0]))}</span></p>
  {/if}
  <div class="plot" bind:clientWidth={width}>
    <svg viewBox="0 0 {width} {height}" {height} role="img" aria-label={label} onpointermove={move} onpointerleave={() => (hover = null)}>
      {#each ticks as tv}
        <line class="grid" x1={PAD.l} x2={PAD.l + plotW} y1={y(tv)} y2={y(tv)} />
        <text class="axis" x={PAD.l - 6} y={y(tv) + 3} text-anchor="end">{format(tv)}</text>
      {/each}
      {#each xticks as tt, i}
        <text class="axis" x={x(tt)} y={height - 5} text-anchor={i === 0 ? 'start' : i === 2 ? 'end' : 'middle'}>{clock(tt)}</text>
      {/each}
      {#each series as s (s.name)}
        {#each paths(s) as d}<path {d} fill="none" stroke={color(s)} stroke-width="2" stroke-linejoin="round" stroke-linecap="round" />{/each}
        {#each singles(s) as p}<circle cx={x(p.t)} cy={y(p.v)} r="3" fill={color(s)} />{/each}
      {/each}
      {#if hover}
        <line class="cross" x1={x(hover.t)} x2={x(hover.t)} y1={PAD.t} y2={PAD.t + plotH} />
        {#each hover.rows as r (r.s.name)}
          {#if r.v !== null}<circle cx={x(hover.t)} cy={y(r.v)} r="4" fill={color(r.s)} stroke="var(--panel)" stroke-width="2" />{/if}
        {/each}
      {/if}
    </svg>
    {#if hover}
      <div class="tip" style="left:{Math.min(x(hover.t) + 10, width - 170)}px">
        <div class="tt">{new Date(hover.t).toLocaleTimeString()}</div>
        {#each hover.rows as r (r.s.name)}
          <div><span class="key" style="background:{color(r.s)}"></span>{r.s.name} <strong>{r.v === null ? 'not reported' : format(r.v)}</strong></div>
        {/each}
      </div>
    {/if}
  </div>
  <details>
    <summary>Data</summary>
    <table>
      <thead><tr><th>Time</th>{#each series as s (s.name)}<th>{s.name}</th>{/each}</tr></thead>
      <tbody>
        {#each [...times].reverse() as t (t)}
          <tr>
            <td>{new Date(t).toLocaleTimeString()}</td>
            {#each series as s (s.name)}{@const v = s.points.find((p) => p.t === t)?.v}<td>{v == null ? '—' : format(v)}</td>{/each}
          </tr>
        {/each}
      </tbody>
    </table>
  </details>
</figure>

<style>
  .chart { margin: 0; display: grid; grid-template-columns: minmax(0, 1fr); gap: 4px; min-width: 0; }
  .legend { list-style: none; margin: 0; padding: 0; display: flex; flex-wrap: wrap; gap: 4px 14px; font-size: var(--sc-t-meta); color: var(--text-dim); }
  .legend li { display: inline-flex; align-items: center; gap: 6px; }
  .key { display: inline-block; width: 10px; height: 3px; border-radius: 2px; margin-right: 4px; vertical-align: middle; }
  .now, .single { font-variant-numeric: tabular-nums; color: var(--text); font-weight: 600; }
  .single { margin: 0; font-size: var(--sc-t-meta); font-weight: 400; color: var(--text-dim); }
  .single .now { font-weight: 600; }
  .plot { position: relative; min-width: 0; overflow: hidden; }
  svg { display: block; width: 100%; touch-action: none; }
  .grid { stroke: var(--sc-hairline, var(--border)); stroke-width: 1; }
  .axis { fill: var(--text-faint); font-size: 10px; font-variant-numeric: tabular-nums; }
  .cross { stroke: var(--text-faint); stroke-width: 1; stroke-dasharray: 2 2; }
  .tip { position: absolute; top: 4px; pointer-events: none; background: var(--panel-raised, var(--panel)); border: 1px solid var(--border); border-radius: var(--radius-sm); padding: 6px 8px; font-size: var(--sc-t-meta); color: var(--text); min-width: 150px; box-shadow: 0 2px 8px rgb(0 0 0 / 0.15); }
  .tt { color: var(--text-faint); margin-bottom: 2px; }
  details { font-size: var(--sc-t-meta); color: var(--text-dim); }
  details table { border-collapse: collapse; margin-top: 4px; font-variant-numeric: tabular-nums; }
  details th, details td { padding: 2px 10px 2px 0; text-align: left; }
</style>
