<script>
  // Who talked to whom, and what the network did about it (#4). The server limits it to the viewer's projects;
  // `pod` narrows it to one, on a pod's and a machine's page.
  import { onDestroy } from 'svelte'
  import { get, timeAgo } from '../api.js'
  import { VERDICTS, verdictWord, verdictTone, dropWord, endWord, portWord, addFlow, flowQuery } from '../flows.js'

  let { ns = '', pod = '', last = 100 } = $props()

  const API = '/api/plugins/k8s/flows'
  let verdict = $state('')
  let follow = $state(false)
  let answer = $state(null)
  let rows = $state([])
  let error = $state('')
  let source = null

  function stop() {
    source?.close()
    source = null
  }

  async function load(project, name, v) {
    stop()
    try {
      const a = await get(`${API}?${flowQuery({ ns: project, pod: name, verdict: v, last })}`)
      answer = a
      rows = [...(a.items || [])].reverse()
      error = ''
    } catch (e) {
      error = e.message
    }
  }

  // Live from here on. EventSource asks again by itself when the server ends the stream, which is when a
  // viewer's rights are looked at again.
  function listen(project, name, v) {
    stop()
    source = new EventSource(`${API}/stream?${flowQuery({ ns: project, pod: name, verdict: v })}`)
    source.onmessage = (m) => {
      rows = addFlow(rows, JSON.parse(m.data))
    }
  }

  $effect(() => {
    const [project, name, v, live] = [ns, pod, verdict, follow]
    load(project, name, v).then(() => {
      if (live && answer?.available) listen(project, name, v)
    })
    return stop
  })
  onDestroy(stop)
</script>

<div class="flows" data-testid="flows">
  <div class="bar">
    <label>Verdict
      <select bind:value={verdict} aria-label="Verdict">
        {#each VERDICTS as [v, word] (v)}<option value={v}>{word}</option>{/each}
      </select>
    </label>
    <label class="check"><input type="checkbox" bind:checked={follow} aria-label="Follow" /> Follow</label>
    <button type="button" onclick={() => load(ns, pod, verdict)} disabled={follow}>Refresh</button>
    <span class="dim count">{rows.length} flow{rows.length === 1 ? '' : 's'}{follow && answer?.available ? ', live' : ''}</span>
  </div>
  {#if error}<p class="bad">{error}</p>{/if}
  {#if answer && !answer.available}
    <p class="dim" data-testid="flows-none">{answer.reason}</p>
  {:else if answer}
    {#if answer.note}<p class="warn" data-testid="flows-note">{answer.note}</p>{/if}
    {#if answer.unavailableNodes?.length}<p class="warn">Not from: {answer.unavailableNodes.join(', ')}</p>{/if}
    {#if answer.partial}<p class="warn">The relay had not finished after 8 seconds: these are the flows it had given by then.</p>{/if}
    {#if answer.error}<p class="bad">{answer.error}</p>{/if}
    {#if rows.length}
      <div class="wrap">
        <table>
          <thead><tr><th>When</th><th>Verdict</th><th>From</th><th>To</th><th>Port</th><th>Node</th></tr></thead>
          <tbody>
            {#each rows as f, i (i)}
              <tr class={verdictTone(f.verdict)}>
                <td class="time" title={f.time ? new Date(f.time).toISOString() : ''}>{f.time ? timeAgo(new Date(f.time).toISOString()) : '—'}</td>
                <td class="verdict">{verdictWord(f.verdict)}{#if f.drop}<div class="dim">{dropWord(f.drop)}</div>{/if}</td>
                <td class="mono" class:other={f.source?.hidden}>{endWord(f.source)}</td>
                <td class="mono" class:other={f.destination?.hidden}>{endWord(f.destination)}</td>
                <td class="mono">{portWord(f)}</td>
                <td class="mono dim">{f.node || ''}</td>
              </tr>
            {/each}
          </tbody>
        </table>
      </div>
    {:else if !answer.note}
      <p class="dim" data-testid="flows-empty">No flows{verdict ? ' with this verdict' : ''} in what the relay holds.</p>
    {/if}
  {/if}
</div>

<style>
  .flows { display: grid; gap: 10px; }
  .bar { display: flex; flex-wrap: wrap; gap: 12px; align-items: center; font-size: var(--sc-t-meta); }
  .bar label { display: inline-flex; gap: 6px; align-items: center; }
  .count { margin-left: auto; }
  .wrap { overflow-x: auto; }
  table { width: 100%; border-collapse: collapse; font-size: var(--sc-t-body); }
  th { text-align: left; font-size: var(--sc-t-meta); font-weight: 500; color: var(--text-dim); padding: var(--sc-row-py) var(--sc-row-px); border-bottom: 1px solid var(--border); white-space: nowrap; }
  td { padding: var(--sc-row-py) var(--sc-row-px); border-bottom: 1px solid var(--sc-hairline); vertical-align: top; }
  .mono { font-family: var(--mono); font-size: var(--sc-t-meta); }
  .dim { color: var(--text-dim); font-size: var(--sc-t-meta); }
  .time { white-space: nowrap; color: var(--text-dim); }
  .verdict { white-space: nowrap; font-weight: 500; }
  tr.ok .verdict { color: var(--ok-strong, var(--text)); }
  tr.bad .verdict { color: var(--bad, #e5484d); }
  .other { color: var(--text-faint); font-style: italic; }
  .warn { color: var(--warn-strong); margin: 0; font-size: var(--sc-t-body); }
  .bad { color: var(--bad, #e5484d); margin: 0; }
  p.dim { margin: 0; font-size: var(--sc-t-body); }
</style>
