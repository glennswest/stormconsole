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
  // A checklist's options, fetched as the viewer when the form opens: they
  // depend on who is looking (your SSH keys), which a creator declared once
  // per plugin cannot know.
  let lists = $state({})

  $effect(() => {
    if (!c) return
    text = c.template || ''
    const v = {}
    for (const f of c.fields || []) v[f.name] = f.default || ''
    values = v
    error = ''
    done = ''
    lists = {}
    for (const f of c.fields || []) {
      if (f.kind !== 'checklist' || !f.source) continue
      lists[f.name] = { loading: true, options: [], note: '' }
      // Left unset until it answers, so a list that never loads is not
      // submitted as "none ticked" — the server's default is all.
      delete values[f.name]
      fetch(f.source)
        .then((r) => (r.ok ? r.json() : Promise.reject(new Error(`${r.status}`))))
        .then((d) => {
          lists[f.name] = { loading: false, options: d.options || [], note: d.note || '' }
          values[f.name] = (d.options || []).filter((o) => o.checked).map((o) => o.value)
        })
        .catch((e) => {
          lists[f.name] = { loading: false, options: [], note: `could not load: ${e.message}` }
        })
    }
  })

  function toggle(name, value, on) {
    const cur = new Set(values[name] || [])
    if (on) cur.add(value)
    else cur.delete(value)
    values[name] = [...cur]
  }

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
        <p class="hint"><span class="mono">{c.method || 'POST'} {c.path}</span> · press ⌘/Ctrl-Enter to create</p>
      {:else}
        <div class="form">
          {#each c.fields as f (f.name)}
            {#if f.kind === 'checklist'}
              <fieldset class="checklist">
                <legend class="lbl">{f.label}</legend>
                {#if lists[f.name]?.loading}
                  <span class="fhint">loading…</span>
                {/if}
                {#each lists[f.name]?.options || [] as o (o.value)}
                  <label class="check">
                    <input
                      type="checkbox"
                      checked={(values[f.name] || []).includes(o.value)}
                      onchange={(e) => toggle(f.name, o.value, e.currentTarget.checked)}
                    />
                    <span class="mono">{o.label || o.value}</span>
                  </label>
                {/each}
                {#if lists[f.name]?.note}<span class="fhint">{lists[f.name].note}</span>{/if}
                {#if f.hint}<span class="fhint">{f.hint}</span>{/if}
              </fieldset>
            {:else}
            <label>
              <span class="lbl">{f.label}{#if f.required}<b>*</b>{/if}</span>
              {#if f.kind === 'select'}
                <select bind:value={values[f.name]}>
                  <!-- The option submits `value` and displays `label`; a
                       plain string option is both, which is what a list of
                       bare strings used to mean. -->
                  {#each f.options as o}
                    {@const value = typeof o === 'string' ? o : o.value}
                    {@const text = typeof o === 'string' ? o : (o.label || o.value)}
                    <option {value}>{value === '' ? '—' : text}</option>
                  {/each}
                </select>
              {:else if f.kind === 'textarea'}
                <textarea bind:value={values[f.name]} rows="4"></textarea>
              {:else}
                <input type={f.kind === 'number' ? 'number' : 'text'} bind:value={values[f.name]} />
              {/if}
              {#if f.hint}<span class="fhint">{f.hint}</span>{/if}
            </label>
            {/if}
          {/each}
        </div>
        <p class="hint"><span class="mono">{c.method || 'POST'} {c.path}</span></p>
      {/if}

      {#if error}<div class="msg err">{error}</div>{/if}
      {#if done}<div class="msg ok">{done}</div>{/if}

      <div class="actions">
        <button class="cancel" onclick={closeCreator}>Cancel</button>
        <button class="go sc-primary" onclick={submit} disabled={busy}>{busy ? 'Creating…' : `Create ${c.label}`}</button>
      </div>
    </div>
  </div>
{/if}

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    background: color-mix(in srgb, var(--bg) 55%, rgb(0 0 0 / 0.65));
    backdrop-filter: blur(2px);
    display: flex;
    align-items: flex-start;
    justify-content: center;
    padding: 6vh 16px;
    z-index: 50;
  }
  .dialog {
    background: var(--panel);
    border: 1px solid var(--border-strong);
    border-radius: var(--radius);
    width: min(780px, 96vw);
    max-height: 88vh;
    overflow: auto;
    padding: 0;
    box-shadow: 0 24px 64px rgb(0 0 0 / 0.45);
  }
  .head {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 14px 18px;
    border-bottom: 1px solid var(--border);
    position: sticky;
    top: 0;
    background: var(--panel);
    z-index: 1;
  }
  h2 { font-size: 16px; font-weight: 600; letter-spacing: -0.01em; margin: 0; }
  .plugin {
    font-size: var(--sc-t-eyebrow);
    text-transform: uppercase;
    letter-spacing: 0.06em;
    color: var(--text-faint);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    padding: 1px 7px;
  }
  .x {
    margin-left: auto;
    background: none;
    border: 1px solid transparent;
    color: var(--text-dim);
    font-size: 13px;
    padding: 3px 8px;
  }
  .x:hover { color: var(--text); background: var(--nav-hover); }

  .desc, .form, textarea, .hint, .msg { margin-left: 18px; margin-right: 18px; }
  .desc { color: var(--text-dim); font-size: var(--sc-t-body); margin-top: 14px; margin-bottom: 12px; }
  textarea, input, select {
    width: 100%;
    box-sizing: border-box;
    background: var(--term-bg);
    color: var(--text);
    border: 1px solid var(--border-strong);
    border-radius: var(--radius-sm);
    padding: 8px 10px;
    font-size: var(--sc-t-body);
  }
  textarea {
    font-family: var(--mono);
    line-height: 1.55;
    resize: vertical;
    margin-top: 14px;
    width: calc(100% - 36px);
    tab-size: 2;
  }
  .form { display: grid; gap: 12px; margin-top: 14px; }
  .form input, .form select { background: var(--panel-raised); }
  label { display: grid; gap: 5px; font-size: var(--sc-t-body); }
  .lbl { color: var(--text-dim); font-weight: 500; }
  .lbl b { color: var(--error); margin-left: 3px; }
  .fhint, .hint { font-size: var(--sc-t-meta); color: var(--text-faint); }
  .fhint { margin: 0; }
  .hint { margin-top: 8px; margin-bottom: 0; }
  .hint .mono { font-family: var(--mono); }

  .msg {
    margin-top: 12px;
    padding: 9px 11px;
    border-radius: var(--radius-sm);
    font-size: var(--sc-t-body);
    white-space: pre-wrap;
  }
  .err { background: var(--error-bg); color: var(--error); border: 1px solid var(--error-border); }
  .ok { background: var(--ok-bg); color: var(--ok); border: 1px solid var(--ok-border); }

  .actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
    margin-top: 18px;
    padding: 14px 18px;
    border-top: 1px solid var(--border);
    background: color-mix(in srgb, var(--panel-raised) 40%, var(--panel));
    position: sticky;
    bottom: 0;
  }
  .cancel { color: var(--text-dim); }
  .go:disabled { opacity: 0.55; }
  fieldset.checklist { border: 0; padding: 0; margin: 0; display: flex; flex-direction: column; gap: 4px; }
  .check { display: flex; gap: 8px; align-items: center; font-size: var(--sc-t-meta); }
  .check input { width: auto; margin: 0; }
</style>
