<script>
  // Placeholder until the phase-3 collector lands: the fleet log viewer
  // will filter by host/severity/search and follow live over SSE.
  import { feed } from '../stores.svelte.js'

  let collector = $derived(feed.components.find((c) => c.id === 'logs:collector'))
</script>

<div class="content">
  <h1>Fleet logs</h1>
  <p class="empty">
    {collector ? collector.detail : 'The fleet log collector is not running yet.'}
  </p>
</div>

<style>
  h1 { font-size: 16px; font-weight: 600; margin-bottom: 12px; }
  .empty { color: var(--text-faint); }
</style>
