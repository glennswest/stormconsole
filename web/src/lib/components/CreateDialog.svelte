<script>
  // The create dialog: a YAML editor seeded with the creator's template,
  // or a form built from its fields. What it posts and where is entirely
  // the creator's declaration — this component knows nothing about pods
  // or volumes.
  import { create, closeCreator } from '../stores.svelte.js'

  const c = $derived(create.open)
  let text = $state('')
  let values = $state({})
  let busy = $state(false)
  let error = $state('')
  let done = $state('')

  $effect(() => {
    if (!c) return
    text = c.template || ''
    const v = {}
    for (const f of c.fields || []) v[f.name] = f.default || ''
    values = v
    error = ''
    done = ''
  })

  function body() {
    if (c.mode === 'yaml') return { type: 'application/yaml', data: text }
    const obj = {}
    for (const f of c.fields || []) {
      let v = values[f.name]
      if (v === '' || v === undefined) {
        if (f.required) throw new Error(`${f.label} is required`)
        continue
      }
      if (f.kind === 'number') v = Number(v)
      if (v === 'true') v = true
      if (v === 'false') v = false
      obj[f.name] = v
    }
    return { type: 'application/json', data: JSON.stringify(obj) }
  }

  async function submit() {
    error = ''
    done = ''
    let b
    try {
      b = body()
    } catch (e) {
      error = e.message
      return
    }
    busy = true
    try {
      const resp = await fetch(c.path, {
        method: c.method || 'POST',
        headers: { 'Content-Type': b.type },
        body: b.data,
      })
      const data = await resp.json().catch(() => ({}))
      if (!resp.ok || data.error) {
        error = data.error || data.message || `${resp.status} ${resp.statusText}`
        if (data.message && data.results) done = ''
      } else {
        done = data.message || `${c.label} created`
        setTimeout(closeCreator, 900)
      }
    } catch (e) {
      error = e.message
    } finally {
      busy = false
    }
  }

  function onkey(e) {
    if (e.key === 'Escape') closeCreator()
    if (e.key === 'Enter' && (e.metaKey || e.ctrlKey)) submit()
  }
</script>

{#if c}
  <div class="backdrop" onclick={closeCreator} onkeydown={onkey} role="presentation">
    <div class="dialog" onclick={(e) => e.stopPropagation()} role="dialog" aria-modal="true" tabindex="-1" onkeydown={onkey}>
      <div class="head">
        <h2>Create {c.label}</h2>
        <span class="plugin">{c.plugin}</span>
        <button class="x" onclick={closeCreator} title="Close">✕</button>
      </div>
      {#if c.description}<p class="desc">{c.description}</p>{/if}

      {#if c.mode === 'yaml'}
        <textarea bind:value={text} spellcheck="false" rows="18"></textarea>
        <p class="hint">Posts to {c.path} · ⌘/Ctrl-Enter to create</p>
      {:else}
        <div class="form">
          {#each c.fields as f (f.name)}
            <label>
              <span class="lbl">{f.label}{#if f.required}<b>*</b>{/if}</span>
              {#if f.kind === 'select'}
                <select bind:value={values[f.name]}>
                  {#each f.options as o}<option value={o}>{o === '' ? '—' : o}</option>{/each}
                </select>
              {:else if f.kind === 'textarea'}
                <textarea bind:value={values[f.name]} rows="4"></textarea>
              {:else}
                <input type={f.kind === 'number' ? 'number' : 'text'} bind:value={values[f.name]} />
              {/if}
              {#if f.hint}<span class="fhint">{f.hint}</span>{/if}
            </label>
          {/each}
        </div>
        <p class="hint">{c.method || 'POST'} {c.path}</p>
      {/if}

      {#if error}<div class="msg err">{error}</div>{/if}
      {#if done}<div class="msg ok">{done}</div>{/if}

      <div class="actions">
        <button class="cancel" onclick={closeCreator}>Cancel</button>
        <button class="go" onclick={submit} disabled={busy}>{busy ? 'Creating…' : 'Create'}</button>
      </div>
    </div>
  </div>
{/if}

<style>
  .backdrop {
    position: fixed; inset: 0; background: rgba(0, 0, 0, 0.55);
    display: flex; align-items: center; justify-content: center; z-index: 50;
  }
  .dialog {
    background: var(--panel); border: 1px solid var(--border); border-radius: var(--radius, 8px);
    width: min(760px, 94vw); max-height: 90vh; overflow: auto; padding: 18px 20px;
    box-shadow: 0 20px 60px rgba(0, 0, 0, 0.4);
  }
  .head { display: flex; align-items: center; gap: 10px; margin-bottom: 6px; }
  h2 { font-size: 16px; font-weight: 600; margin: 0; }
  .plugin { font-size: 11px; color: var(--text-faint); border: 1px solid var(--border); border-radius: 10px; padding: 1px 8px; }
  .x { margin-left: auto; background: none; border: none; color: var(--text-dim); font-size: 14px; cursor: pointer; }
  .desc { color: var(--text-dim); font-size: 13px; margin: 0 0 10px; }
  textarea, input, select {
    width: 100%; box-sizing: border-box; background: var(--panel-raised); color: var(--text);
    border: 1px solid var(--border); border-radius: var(--radius-sm, 4px); padding: 6px 8px; font-size: 13px;
  }
  textarea { font-family: ui-monospace, SFMono-Regular, Menlo, monospace; line-height: 1.4; resize: vertical; }
  .form { display: grid; gap: 10px; }
  label { display: grid; gap: 4px; font-size: 13px; }
  .lbl { color: var(--text-dim); }
  .lbl b { color: var(--error); margin-left: 3px; }
  .fhint, .hint { font-size: 11.5px; color: var(--text-faint); }
  .hint { margin: 8px 0 0; }
  .msg { margin-top: 10px; padding: 8px 10px; border-radius: var(--radius-sm, 4px); font-size: 13px; white-space: pre-wrap; }
  .err { background: var(--error-bg, rgba(220, 60, 60, 0.12)); color: var(--error); border: 1px solid var(--error-border, var(--error)); }
  .ok { background: rgba(60, 180, 90, 0.12); color: var(--ok); border: 1px solid var(--ok); }
  .actions { display: flex; justify-content: flex-end; gap: 8px; margin-top: 14px; }
  .cancel { background: none; border: 1px solid var(--border); color: var(--text-dim); padding: 6px 12px; border-radius: var(--radius-sm, 4px); cursor: pointer; }
  .go { background: var(--accent); color: var(--accent-fg, #fff); border: none; padding: 6px 14px; border-radius: var(--radius-sm, 4px); font-weight: 600; cursor: pointer; }
  .go:disabled { opacity: 0.6; }
</style>
