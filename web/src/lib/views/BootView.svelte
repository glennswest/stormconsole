<script>
  // The boot and the flow-over, to watch and to record (#271): the node's own story, full page, large type,
  // nothing on it to press. Its own dark palette, so a recording looks the same whichever theme is chosen.
  // Everything is read here and nothing kept: PID 1's timeline from system-data, and the engine's open health.
  import { onDestroy } from 'svelte'
  import { get, formatBytes } from '../api.js'
  import { bootStory, uptimeMs, flowStory, flowTotal, rates, sparkline, clock, seconds } from '../boot.js'

  const KEEP = 150 // five minutes at one reading every two seconds

  let story = $state(null)
  let bootNote = $state('')
  let health = $state(null)
  let healthError = $state('')
  let samples = $state([])
  let watched = $state(null)
  let now = $state(Date.now())

  async function loadBoot() {
    try {
      const status = await get('/api/v1/history')
      if (!status.available) {
        bootNote = status.note || 'This node keeps no record of its boots.'
        return
      }
      const list = await get('/api/v1/history/boots')
      const rows = list.items || list.boots || list
      const row = (rows || []).find((b) => b.current) || (rows || [])[0]
      if (!row) {
        bootNote = 'This node has no record of a boot yet.'
        return
      }
      story = bootStory(await get(`/api/v1/history/boots/${encodeURIComponent(row.id)}`))
      bootNote = story.current ? '' : 'The newest boot on record, not this one: PID 1 has not written this boot yet.'
    } catch (e) {
      bootNote = /404/.test(e.message) ? 'This node keeps no record of its boots: it has no system-data volume.' : e.message
    }
  }

  async function loadFlow() {
    try {
      const h = await get('/api/plugins/sb/proxy/api/v1/health')
      health = h
      healthError = ''
      if (h.flow_over) samples = [...samples, { t: Date.now(), bytes: h.flow_over.bytes }].slice(-KEEP)
      if (h.flow_over && h.flow_over_remaining > 0) watched = { seconds: h.flow_over.seconds, average: h.flow_over.mb_per_s }
    } catch (e) {
      healthError = e.message
    }
  }

  loadBoot()
  loadFlow()
  const timers = [
    setInterval(loadBoot, 5000),
    setInterval(loadFlow, 2000),
    setInterval(() => (now = Date.now()), 500),
  ]
  onDestroy(() => timers.forEach(clearInterval))

  const flow = $derived(healthError ? { state: 'unknown', word: 'The engine cannot be asked', sub: healthError } : flowStory(health))
  const speeds = $derived(rates(samples))
  const speed = $derived(speeds.length ? speeds[speeds.length - 1] : null)
  const line = $derived(sparkline(speeds))
  const up = $derived(uptimeMs(story, now))
  const big = $derived(flow.percent != null && ['moving', 'checking', 'stalled', 'settling', 'corrupt'].includes(flow.state))
  const total = $derived(flowTotal(flow, watched))
  const mbps = (v) => (v == null ? '—' : v >= 100 ? Math.round(v).toString() : v.toFixed(1))
</script>

<div class="boot" data-state={flow.state}>
  <a class="out" href="#/">Console</a>
  <header>
    <div>
      <h1 data-testid="host">{story?.hostname || 'This node'}</h1>
      <p class="sub">{[story?.release && `StormCOS ${story.release}`, story?.kernel].filter(Boolean).join(' · ')}</p>
    </div>
    {#if up != null}
      <div class="clock" data-testid="clock"><span class="digits">{clock(up)}</span><span class="what">since the kernel started</span></div>
    {/if}
  </header>

  <main class="cols">
    <section class="card" data-testid="boot-card">
      <h2>Boot</h2>
      {#if bootNote}<p class="note">{bootNote}</p>{/if}
      {#if story}
        <ol class="phases">
          {#each story.rows as r (r.name + r.at)}
            <li>
              <span class="name">{r.name}</span>
              <span class="track"><span class="bar" class:before={r.before} style="left:{(r.at / story.span) * 100}%;width:{Math.max(0.6, (r.ms / story.span) * 100)}%"></span></span>
              <span class="secs">{seconds(r.ms)}</span>
            </li>
          {/each}
        </ol>
        {#if story.more}<p class="note">and {story.more} more on the History page</p>{/if}
        <div class="done" class:waiting={story.allRunningMs == null} data-testid="all-running">
          {#if story.allRunningMs != null}
            <span class="digits">{seconds(story.allRunningMs)}</span>
            <span class="what">to every service running{story.allRunningDetail ? `, ${story.allRunningDetail}` : ''}</span>
          {:else}
            <span class="what">{story.started} service{story.started === 1 ? '' : 's'} started, still coming up</span>
          {/if}
          {#if story.exited || story.refused}<span class="warn">{story.exited} exited · {story.refused} refused</span>{/if}
        </div>
      {/if}
    </section>

    <section class="card" data-testid="flow-card">
      <h2>Flow-over</h2>
      <p class="word" data-testid="flow-word">{flow.word}</p>
      {#if flow.sub}<p class="note" data-testid="flow-sub">{flow.sub}</p>{/if}
      {#if big}
        <div class="progress" role="progressbar" aria-valuenow={Math.round(flow.percent)} aria-valuemin="0" aria-valuemax="100"><span style="width:{flow.percent}%"></span></div>
        <div class="stats">
          <div><span class="digits" data-testid="flow-percent">{Math.floor(flow.percent)}%</span><span class="what">moved</span></div>
          <div><span class="digits" data-testid="flow-speed">{mbps(speed ?? flow.average)}</span><span class="what">MB/s{speed == null ? ', average' : ''}</span></div>
          <div><span class="digits">{flow.eta != null && flow.left ? clock(flow.eta * 1000) : '—'}</span><span class="what">left</span></div>
        </div>
        {#if line}
          <svg class="spark" viewBox="0 0 600 80" preserveAspectRatio="none" role="img" aria-label="MB per second over the last minutes" data-testid="flow-spark"><polyline points={line} /></svg>
        {/if}
        <p class="note">{flow.moved.toLocaleString()} extents moved · {flow.zeroed.toLocaleString()} all zeros · {(flow.left ?? 0).toLocaleString()} left · {formatBytes(flow.bytes)} copied</p>
      {:else if total}
        <div class="stats" data-testid="flow-total">
          <div><span class="digits">{formatBytes(total.bytes)}</span><span class="what">copied</span></div>
          {#if total.seconds != null}
            <div><span class="digits">{clock(total.seconds * 1000)}</span><span class="what">it took</span></div>
            <div><span class="digits">{mbps(total.average)}</span><span class="what">MB/s on average</span></div>
          {/if}
        </div>
      {/if}
    </section>
  </main>
</div>

<style>
  .boot {
    --b-bg: #0b0f14;
    --b-panel: #131a22;
    --b-line: #243040;
    --b-text: #eef3f8;
    --b-dim: #8fa1b3;
    --b-accent: #5cc8ff;
    --b-before: #54657a;
    --b-ok: #4fd18b;
    --b-warn: #f2b84b;
    --b-bad: #ff6b6b;
    position: fixed;
    inset: 0;
    display: flex;
    flex-direction: column;
    gap: clamp(12px, 2vh, 28px);
    padding: clamp(16px, 3vh, 44px) clamp(16px, 3vw, 56px);
    background: var(--b-bg);
    color: var(--b-text);
    overflow: auto;
  }
  .out { position: absolute; top: 8px; right: 12px; color: var(--b-dim); font-size: 13px; opacity: 0; }
  .out:hover, .out:focus { opacity: 1; }
  header { display: flex; justify-content: space-between; align-items: flex-end; gap: 24px; flex-wrap: wrap; }
  h1 { margin: 0; font-size: clamp(28px, 5vh, 60px); font-weight: 600; letter-spacing: -0.01em; }
  .sub { margin: 4px 0 0; color: var(--b-dim); font-size: clamp(15px, 2.2vh, 26px); }
  .digits { font-variant-numeric: tabular-nums; font-weight: 600; line-height: 1; white-space: nowrap; }
  .what { display: block; color: var(--b-dim); font-size: clamp(13px, 1.8vh, 20px); margin-top: 6px; }
  .clock { text-align: right; }
  .clock .digits { font-size: clamp(40px, 9vh, 110px); color: var(--b-accent); }
  .cols { flex: 1; min-height: 0; display: grid; grid-template-columns: repeat(auto-fit, minmax(min(100%, 460px), 1fr)); gap: clamp(12px, 2vw, 32px); }
  .card { background: var(--b-panel); border: 1px solid var(--b-line); border-radius: 10px; padding: clamp(14px, 2.6vh, 32px); display: flex; flex-direction: column; gap: clamp(8px, 1.6vh, 20px); min-width: 0; }
  h2 { margin: 0; color: var(--b-dim); font-size: clamp(13px, 1.9vh, 22px); font-weight: 600; text-transform: uppercase; letter-spacing: 0.08em; }
  .note { margin: 0; color: var(--b-dim); font-size: clamp(14px, 2vh, 22px); }
  .phases { list-style: none; margin: 0; padding: 0; display: grid; gap: clamp(6px, 1.2vh, 14px); }
  .phases li { display: grid; grid-template-columns: 36% 1fr 4.5em; gap: 12px; align-items: center; font-size: clamp(14px, 2.2vh, 24px); }
  .name { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .secs { text-align: right; font-variant-numeric: tabular-nums; color: var(--b-dim); }
  .track { position: relative; height: clamp(8px, 1.4vh, 16px); background: var(--b-line); border-radius: 4px; overflow: hidden; }
  .bar { position: absolute; top: 0; bottom: 0; background: var(--b-accent); border-radius: 4px; }
  .bar.before { background: var(--b-before); }
  .done { margin-top: auto; padding-top: clamp(8px, 1.6vh, 20px); border-top: 1px solid var(--b-line); }
  .done .digits { font-size: clamp(32px, 7vh, 84px); color: var(--b-ok); }
  .warn { display: block; margin-top: 6px; color: var(--b-warn); font-size: clamp(14px, 2vh, 22px); }
  .word { margin: 0; font-size: clamp(24px, 5.2vh, 60px); font-weight: 600; line-height: 1.1; }
  [data-state='local'] .word { color: var(--b-ok); }
  [data-state='moving'] .word, [data-state='checking'] .word, [data-state='settling'] .word { color: var(--b-accent); }
  [data-state='stalled'] .word, [data-state='corrupt'] .word { color: var(--b-bad); }
  .progress { height: clamp(12px, 2.4vh, 28px); background: var(--b-line); border-radius: 8px; overflow: hidden; }
  .progress span { display: block; height: 100%; background: var(--b-accent); transition: width 1.8s linear; }
  [data-state='stalled'] .progress span, [data-state='corrupt'] .progress span { background: var(--b-bad); }
  .stats { display: grid; grid-template-columns: repeat(3, 1fr); gap: 12px; }
  .stats .digits { font-size: clamp(26px, 6.4vh, 76px); }
  .spark { width: 100%; flex: 1; min-height: 48px; }
  .spark polyline { fill: none; stroke: var(--b-accent); stroke-width: 2.5; vector-effect: non-scaling-stroke; }
  @media (prefers-reduced-motion: reduce) { .progress span { transition: none; } }
</style>
