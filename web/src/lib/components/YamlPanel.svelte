<script>
  // The YAML tab every OpenShift resource page has: the object as the
  // apiserver holds it, and — where the plugin will take a write — an
  // edit that saves back.
  //
  // Saving is a replace, so the resourceVersion the document was loaded
  // with is the concurrency guard: an edit of something changed since
  // comes back 409, and that is reported as what it is rather than as a
  // generic failure.
  import Icon from './Icon.svelte'

  let { yaml = '', savePath = '', label = 'object' } = $props()

  let editing = $state(false)
  let draft = $state('')
  let busy = $state(false)
  let error = $state('')
  let saved = $state('')

  function start() {
    draft = yaml
    error = ''
    saved = ''
    editing = true
  }

  function cancel() {
    editing = false
    error = ''
  }

  async function save() {
    busy = true
    error = ''
    saved = ''
    try {
      const resp = await fetch(savePath, {
        method: 'PUT',
        headers: { 'Content-Type': 'application/yaml' },
        body: draft,
      })
      const data = await resp.json().catch(() => ({}))
      if (!resp.ok) throw new Error(data.error || `${resp.status} ${resp.statusText}`)
      saved = data.message || 'saved'
      editing = false
    } catch (e) {
      error = e.message
    }
    busy = false
  }
</script>

<div class="yaml">
  <div class="bar">
    <span class="what">{label}</span>
    {#if saved}<span class="saved">{saved}</span>{/if}
    <span class="right">
      {#if savePath && !editing}
        <button onclick={start}><Icon name="table" size={13} /> Edit</button>
      {:else if editing}
        <button class="sc-primary" disabled={busy} onclick={save}>{busy ? 'Saving…' : 'Save'}</button>
        <button disabled={busy} onclick={cancel}>Cancel</button>
      {/if}
    </span>
  </div>

  {#if error}
    <p class="error">{error}</p>
  {/if}

  {#if editing}
    <textarea bind:value={draft} spellcheck="false" aria-label="Edit {label}"></textarea>
  {:else}
    <pre>{yaml}</pre>
  {/if}
</div>

<style>
  .yaml {
    background: var(--panel);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    overflow: hidden;
  }
  .bar {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 7px var(--sc-row-px);
    border-bottom: 1px solid var(--border);
    background: color-mix(in srgb, var(--panel-raised) 55%, var(--panel));
  }
  .what {
    font-size: var(--sc-t-eyebrow);
    text-transform: uppercase;
    letter-spacing: 0.06em;
    color: var(--text-faint);
  }
  .saved { font-size: var(--sc-t-meta); color: var(--ok); }
  .right { margin-left: auto; display: flex; gap: 6px; }
  .right button { font-size: var(--sc-t-meta); padding: 3px 10px; }

  .error {
    margin: 0;
    padding: 8px var(--sc-row-px);
    font-size: var(--sc-t-body);
    color: var(--error);
    background: var(--error-bg, transparent);
    border-bottom: 1px solid var(--border);
  }

  pre,
  textarea {
    margin: 0;
    display: block;
    width: 100%;
    max-height: 60vh;
    overflow: auto;
    padding: 12px var(--sc-row-px);
    font-family: var(--mono);
    font-size: var(--sc-t-meta);
    line-height: 1.55;
    color: var(--text);
    background: transparent;
    border: none;
    white-space: pre;
    tab-size: 2;
  }
  textarea {
    height: 60vh;
    resize: vertical;
    white-space: pre;
    outline: none;
  }
</style>
