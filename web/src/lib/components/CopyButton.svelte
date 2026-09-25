<script>
  // Copy one value to the clipboard: an address, a MAC. Says it worked,
  // because a click that changes nothing on screen reads as a click that
  // did nothing.
  //
  // `navigator.clipboard` exists only in a secure context, and the console
  // is often reached over plain http on a node's address, so the old
  // selection-and-execCommand path is the fallback rather than an error.
  let { value, label = 'Copy' } = $props()
  let done = $state(false)

  async function copy(e) {
    e.stopPropagation()
    try {
      await navigator.clipboard.writeText(value)
    } catch {
      const t = document.createElement('textarea')
      t.value = value
      t.setAttribute('readonly', '')
      t.style.position = 'fixed'
      t.style.opacity = '0'
      document.body.appendChild(t)
      t.select()
      try { document.execCommand('copy') } catch {}
      t.remove()
    }
    done = true
    setTimeout(() => (done = false), 1200)
  }
</script>

<button class="copy" class:done onclick={copy} title={`${label} ${value}`} aria-label={`${label} ${value}`}>
  {done ? 'copied' : 'copy'}
</button>

<style>
  .copy {
    font-size: var(--sc-t-eyebrow);
    padding: 0 6px;
    line-height: 1.6;
    margin-left: 6px;
    color: var(--text-dim);
    background: none;
    border: 1px solid var(--sc-hairline, var(--border));
    border-radius: var(--radius);
    cursor: pointer;
    vertical-align: baseline;
  }
  .copy:hover { color: var(--text); border-color: var(--border); }
  .copy.done { color: var(--ok); border-color: var(--ok); }
</style>
