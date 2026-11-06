<script>
  // How many replicas, on the row (#36). Zero stops it, so that one is asked about.
  import { call } from '../api.js'

  let { action, label, current = 0, ondone, onerror } = $props()
  let edited = $state(null)
  let busy = $state(false)
  const value = $derived(edited ?? current)

  async function go() {
    const n = Number(value)
    if (n === 0 && current !== 0 && !confirm(`Stop every replica of ${label}?`)) return
    busy = true
    try {
      ondone?.(await call(action.method, action.path, undefined, { replicas: n }), n)
      edited = null
    } catch (e) {
      if (e.message !== 'cancelled') onerror?.(e)
    }
    busy = false
  }
</script>

<span class="scale">
  <input
    type="number"
    min="0"
    max="10000"
    step="1"
    aria-label={`Replicas for ${label}`}
    value={value}
    oninput={(e) => (edited = e.currentTarget.value)}
  />
  <button disabled={busy || value === '' || Number(value) === current} onclick={go}>Scale</button>
</span>

<style>
  .scale { display: inline-flex; gap: 4px; align-items: center; }
  input { width: 62px; }
</style>
