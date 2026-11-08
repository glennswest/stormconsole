<script>
  // Confirm one of a RAID controller's verbs (#325): Switch to HBA, Set good,
  // Set JBOD. Each is a drive operation (storage-admin, as the person, #82):
  // the console names the word to type — a drive's serial, the controller's
  // card for the controller — and the request carries it. Nothing here
  // destroys data, and the dialog says what does happen (a reboot for HBA).
  import { call, get } from '../api.js'
  import { noteActivity } from '../stores.svelte.js'

  /// `ask`: {title, verb, lines: [what happens], path, body, source, done(r) → sentence}
  let { ask, onclose, ondone } = $props()

  let guard = $state(null)
  let guardError = $state('')
  let typed = $state('')
  let busy = $state(false)
  let error = $state('')

  $effect(() => {
    get(`/api/v1/console/guard?method=POST&path=${encodeURIComponent(ask.path)}`)
      .then((g) => (guard = g.guard))
      .catch((e) => (guardError = e.message))
  })
  const word = $derived(guard?.confirm || '')
  const refused = $derived(guard && !guard.allowed ? guard.reason || 'not allowed' : guardError)

  async function run() {
    busy = true
    error = ''
    try {
      const r = await call('POST', ask.path, typed.trim(), ask.body)
      const said = ask.done ? ask.done(r) : `${ask.verb}: done`
      noteActivity({ reason: ask.verb, message: said, source: ask.source })
      ondone?.(said, r)
    } catch (e) {
      if (e.message !== 'cancelled') {
        error = e.message
        noteActivity({ reason: ask.verb, message: `refused: ${e.message}`, source: ask.source, warning: true })
      }
    }
    busy = false
  }
  function onkey(e) {
    if (e.key === 'Escape' && !busy) onclose?.()
  }
</script>

<svelte:window onkeydown={onkey} />

<div class="scrim">
  <div class="dialog" role="dialog" aria-modal="true" aria-label={ask.title} data-testid="controller-dialog">
    <header>
      <h2>{ask.title}</h2>
      <button class="x" aria-label="Close" disabled={busy} onclick={() => onclose?.()}>×</button>
    </header>
    <div class="body">
      <ul class="steps" data-testid="controller-steps">
        {#each ask.lines as l}<li>{l}</li>{/each}
      </ul>
      {#if refused}
        <p class="refusal" data-testid="controller-refused">{refused}</p>
      {:else if !guard}
        <p class="dim">Asking whether you may…</p>
      {:else}
        <label class="type"><span>Type <span class="mono" data-testid="controller-word">{word}</span> to confirm</span>
          <input bind:value={typed} aria-label="Confirmation" autocomplete="off" />
        </label>
        <p class="dim">Done as you; only a storage-admin may.</p>
      {/if}
      {#if error}<p class="error" data-testid="controller-error">{error}</p>{/if}
    </div>
    <footer>
      <button disabled={busy} onclick={() => onclose?.()}>Cancel</button>
      <button class="primary" disabled={busy || !!refused || !word || typed.trim() !== word} onclick={run} data-testid="controller-go">{ask.verb}</button>
    </footer>
  </div>
</div>

<style>
  .scrim { position: fixed; inset: 0; z-index: 90; background: rgb(0 0 0 / 0.45); display: grid; place-items: start center; padding: 40px 8px; overflow-y: auto; }
  .dialog { width: min(620px, 100%); background: var(--panel); border: 1px solid var(--border-strong); border-radius: var(--radius); box-shadow: 0 12px 40px rgb(0 0 0 / 0.35); display: grid; }
  header { display: flex; align-items: center; gap: 10px; padding: 12px 16px; border-bottom: 1px solid var(--border); }
  h2 { font-size: 15px; margin: 0; overflow-wrap: anywhere; }
  .x { background: none; border: none; font-size: 20px; line-height: 1; padding: 0 4px; color: var(--text-dim); margin-left: auto; }
  .body { padding: 8px 16px 12px; font-size: var(--sc-t-body); }
  .steps { margin: 4px 0; padding-left: 20px; display: grid; gap: 4px; }
  .refusal { margin: 10px 0; padding: 8px 12px; border-left: 3px solid var(--warn-strong); background: var(--panel-raised); overflow-wrap: anywhere; }
  .dim { color: var(--text-dim); font-size: var(--sc-t-meta); }
  .error { color: var(--error); margin: 8px 0 0; overflow-wrap: anywhere; }
  footer { display: flex; flex-wrap: wrap; gap: 8px; justify-content: flex-end; padding: 10px 16px; border-top: 1px solid var(--border); }
  .type { display: grid; gap: 4px; margin: 12px 0 0; max-width: 360px; }
  .mono { font-family: var(--mono); }
</style>
