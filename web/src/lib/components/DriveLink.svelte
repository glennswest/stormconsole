<script>
  // A drive's SAS link (#339, stormdrive#149): the negotiated rate against the
  // maximum, the four counters for the drive's side and the host's (or the
  // expander's), each with its rate per minute, what stormdrive recommends,
  // and the loss-of-sync trend: the last 24 h from ironprom, else the last
  // hour's samples this console kept from the Drive object.
  import { onDestroy } from 'svelte'
  import LineChart from './LineChart.svelte'
  import { ago } from '../ui/time.js'
  import { COUNTERS, rateLine, counterRows, total, perMin, levelWords, levelTone, trendFromHistory } from '../drivelink.js'

  let { row = null, level = null, serial = '', local = true } = $props()

  const link = $derived(row?.link || null)
  const rows = $derived(counterRows(row))
  const tone = $derived(levelTone(level))

  // The trend: ironprom holds this node's own series only, so another
  // node's drive goes straight to the kept samples.
  let trend = $state(null)
  let source = $state('')
  let timer = null
  async function load() {
    let series = []
    let from = 0
    let to = 0
    if (local && serial) {
      try {
        const p = new URLSearchParams({ entity: 'drive', serial, only: 'link', window: '86400' })
        const r = await fetch(`/api/plugins/metrics/charts?${p}`)
        if (r.ok) {
          const body = await r.json()
          const c = (body.charts || []).find((x) => x.id === 'link_sync')
          if (c && c.series.some((s) => s.points.some((pt) => pt.v !== null))) {
            series = c.series
            from = body.from
            to = body.to
            source = 'the last 24 hours, from this node’s ironprom'
          }
        }
      } catch {}
    }
    if (!series.length) {
      series = trendFromHistory(row?.history, row?.hostSide)
      const pts = row?.history || []
      if (pts.length) {
        from = pts[0].at * 1000
        to = pts[pts.length - 1].at * 1000
        source = local
          ? `the last ${pts.length} samples this console kept from the Drive object (ironprom has none)`
          : `the last ${pts.length} samples this console kept from the Drive object (ironprom holds only this node’s series)`
      } else source = ''
    }
    trend = series.length ? { series, from, to } : null
    clearTimeout(timer)
    timer = setTimeout(load, 60000)
  }
  $effect(() => {
    row
    serial
    load()
  })
  onDestroy(() => clearTimeout(timer))
</script>

<section class="link" data-testid="drive-link">
  <h3>
    Link
    {#if level}<span class="level {tone}" data-testid="link-level">{level}</span>{/if}
    {#if link?.since}<span class="dim small">since {ago(new Date(link.since * 1000).toISOString())} ago</span>{/if}
  </h3>
  {#if !link}
    <p class="dim">
      {#if level}
        Its node’s stormdrive says the link is {level}, but sends no counters: it predates the Drive object’s link (stormdrive v0.49.0).
      {:else}
        No SAS link reported for this drive: it is not on a SAS card, or its node’s stormdrive predates v0.49.0.
      {/if}
    </p>
  {:else}
    <p data-testid="link-rate"><strong>{rateLine(link)}</strong>{#if link.policy}<span class="dim"> · policy {link.policy}: set by a person, held every boot</span>{/if}{#if link.hostPhy}<span class="dim"> · {row.hostSide} phy {link.hostPhy}</span>{/if}</p>
    <p class="dim small">{levelWords(level)}</p>
    {#if level === 'degraded' || level === 'failing'}
      <p class="{tone}" data-testid="link-message">
        {link.message}{#if link.recommendation}. <strong>{link.recommendation}</strong>{/if}
      </p>
    {/if}
    {#if rows.length}
      <table class="counters">
        <thead>
          <tr><th>Side</th><th>Phy</th><th>Rate</th>{#each COUNTERS as [, word]}<th class="n">{word}</th>{/each}</tr>
        </thead>
        <tbody>
          {#each rows as r (r.side + r.phy)}
            <tr>
              <td>{r.side}</td>
              <td class="mono">{r.phy}</td>
              <td class="mono">{r.rate ? `${r.rate / 1000} Gbit` : 'no link'}</td>
              {#each r.counters as c (c.key)}
                <td class="n" class:moving={c.moving} title="lifetime total {total(c.total)}">
                  <span class="rate">{perMin(c.rate)}</span>
                  <span class="dim small tot">{total(c.total)}</span>
                </td>
              {/each}
            </tr>
          {/each}
        </tbody>
      </table>
      <p class="dim small">Each counter: its rate per minute over the last sample, and below it the lifetime total, which is history (a saturated counter has stopped counting). The rate is what says now.</p>
    {/if}
  {/if}
  {#if trend}
    <h4>Loss of sync per minute</h4>
    <LineChart series={trend.series} from={trend.from} to={trend.to} format={(v) => perMin(v)} label="Loss of sync per minute" height={120} />
    <p class="dim small" data-testid="link-trend-source">From {source}.</p>
  {:else if link}
    <p class="dim small">No trend yet: ironprom has no series for this drive and the console has kept no sample.</p>
  {/if}
</section>

<style>
  .link { margin: 10px 0; }
  h3 { display: flex; gap: 8px; align-items: baseline; }
  h4 { margin: 10px 0 4px; font-size: var(--sc-t-meta); font-weight: 600; }
  .level { font-size: var(--sc-t-meta); padding: 0 6px; border-radius: 3px; border: 1px solid var(--border); color: var(--text-dim); font-weight: 600; }
  .level.warn { color: var(--warn-strong); border-color: var(--warn-strong); }
  .level.error { color: var(--error); border-color: var(--error); }
  p.warn { color: var(--warn-strong); }
  p.error { color: var(--error); }
  .dim { color: var(--text-dim); }
  .small { font-size: var(--sc-t-meta); }
  .mono { font-family: var(--mono); }
  table.counters { border-collapse: collapse; margin: 6px 0; width: 100%; }
  .counters th { text-align: left; font-size: var(--sc-t-meta); color: var(--text-faint); font-weight: 400; padding: 2px 8px; }
  .counters td { padding: 3px 8px; border-top: 1px solid var(--sc-hairline, var(--border)); vertical-align: top; }
  .n { text-align: right; }
  .rate { display: block; font-family: var(--mono); }
  .tot { display: block; font-family: var(--mono); }
  td.moving .rate { color: var(--error); font-weight: 600; }
</style>
