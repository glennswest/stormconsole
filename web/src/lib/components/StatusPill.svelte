<script>
  // Health as an icon *and* a word. A colour alone fails colour-blind
  // operators and prints as grey; the glyph carries the same information.
  let { health = 'unknown', label = null, size = 12 } = $props()

  const GLYPH = { ok: '✓', warn: '!', error: '✕', idle: '–', unknown: '?' }
  const WORD = { ok: 'Ready', warn: 'Degraded', error: 'Failed', idle: 'Idle', unknown: 'Unknown' }
  const state = $derived(GLYPH[health] ? health : 'unknown')
</script>

<span class="sc-status {state}" title={WORD[state]}>
  <span class="mark" style="width:{size}px;height:{size}px;font-size:{size - 3}px">{GLYPH[state]}</span>
  {#if label !== ''}{label ?? WORD[state]}{/if}
</span>

<style>
  .mark {
    display: inline-grid;
    place-items: center;
    border-radius: 50%;
    border: 1px solid currentColor;
    font-weight: 700;
    line-height: 1;
    flex-shrink: 0;
  }
  .ok .mark { background: var(--ok-bg); }
  .warn .mark { background: var(--warn-bg); }
  .error .mark { background: var(--error-bg); }
</style>
