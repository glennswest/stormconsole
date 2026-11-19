<script>
  // A drive's format log (#335, stormdrive#140): every sense, retry, refused
  // fast format and fallback stormdrive logged, with times. The error lines are
  // always shown; the whole log (the newest 500 lines) folds open below them.
  //
  // The Drive object carries only the newest 20 lines, and a 19 h format logs
  // its progress every 30 s, so a refused fast format at the start leaves that
  // tail within minutes. The whole log is read instead, again each time the
  // tail gains a line (the page's own poll brings the tail), and once more
  // when the run ends. The log is stormdrive's: a reload reads it again.
  import { get } from '../api.js'
  import { errorLines, logMark, logTone } from '../driveformat.js'

  let { r } = $props()

  let lines = $state(null)
  let error = $state('')
  const f = $derived(r.format)
  const mark = $derived(logMark(f))
  const shown = $derived(lines ?? f.logTail ?? [])
  const errors = $derived(errorLines(shown))

  let asked = ''
  $effect(() => {
    const m = mark
    if (m === asked) return
    asked = m
    read()
  })

  async function read() {
    try {
      const v = await get(`/api/plugins/drive/formats/log?node=${encodeURIComponent(r.node)}&id=${encodeURIComponent(r.id)}`)
      lines = v.lines
      error = ''
    } catch (e) {
      // The tail still shows; say that the rest was not read.
      error = e.message
    }
  }

  const clock = (t) => (t ? new Date(t * 1000).toLocaleTimeString() : '—')
</script>

<div class="log" data-testid="format-log">
  {#each errors as l, i (i)}
    <p class="line {logTone(l.line)}" data-testid="format-log-error"><time>{clock(l.atUnix)}</time> {l.line}</p>
  {/each}
  {#if shown.length}
    <details>
      <summary data-testid="format-log-toggle">Log · {shown.length} {shown.length === 1 ? 'line' : 'lines'}{lines ? '' : ', the newest'}{errors.length ? '' : ', no errors'}</summary>
      <div class="all" data-testid="format-log-all">
        {#each shown as l, i (i)}
          <p class="line {logTone(l.line)}"><time>{clock(l.atUnix)}</time> {l.line}</p>
        {/each}
      </div>
    </details>
  {/if}
  {#if error}<p class="dim">The whole log was not read: {error}</p>{/if}
</div>

<style>
  .log { margin-top: 4px; font-size: var(--sc-t-meta); }
  .line { margin: 2px 0 0; font-family: var(--mono); white-space: pre-wrap; overflow-wrap: anywhere; color: var(--text-dim); }
  .line.warn { color: var(--warn-strong); }
  .line.bad { color: var(--error); }
  time { color: var(--text-faint); }
  summary { cursor: pointer; color: var(--text-dim); margin-top: 4px; }
  .all { max-height: 320px; overflow: auto; border: 1px solid var(--border); border-radius: 4px; padding: 4px 8px; margin-top: 4px; }
  .dim { color: var(--text-dim); margin: 2px 0 0; }
</style>
