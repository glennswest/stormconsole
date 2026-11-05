<script>
  // What an image was made from (#85), from the node's registry. Only a source the
  // catalog records is said: a component's digest there is its own, not an upstream's.
  import { get } from '../api.js'

  let { name = '', gap = '' } = $props()
  let answer = $state(null)
  let error = $state('')
  $effect(() => {
    if (!name) return
    get(`/api/plugins/reg/origin?${new URLSearchParams({ name })}`)
      .then((a) => (answer = a))
      .catch((e) => (error = e.message))
  })
</script>

{#if !name}
  <span class="dim">no image is named, so there is nothing to ask the registry</span>
{:else if error}
  <span class="dim">the node's registry was not read: {error}</span>
{:else if answer?.error}
  <span class="dim">the node's registry did not answer: {answer.error}</span>
{:else if answer && !answer.found}
  <span class="dim">this node's registry does not list this image</span>
{:else if answer?.source}
  <span class="mono">{answer.source}</span>
  <div class="dim mono wrap">{answer.upstreamDigest || 'upstream digest not recorded'}</div>
{:else if answer}
  <span class="dim">no upstream recorded: built here, or its origin is not recorded yet ({gap})</span>
  {#if answer.releases?.length}<div class="dim">in {answer.releases.join(', ')}</div>{/if}
{:else}
  <span class="dim">asking the registry…</span>
{/if}

<style>
  .dim { color: var(--text-dim); }
  .mono { font-family: var(--mono); font-size: var(--sc-t-meta); }
  .wrap { overflow-wrap: anywhere; }
</style>
