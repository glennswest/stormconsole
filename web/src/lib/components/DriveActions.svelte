<script>
  // A drive's commands as one menu (#308). A drive carries a dozen actions,
  // and a row of buttons ran past the edge of the screen, Format with it.
  // Here they are grouped by what they are for, the destructive ones last and
  // marked, and the menu is placed inside the window whatever its width.
  //
  // `actions` are the feed's (one drive's, or for a selection those every
  // drive offers, with `count`: how many of the selection have each enabled).
  // `onformat` opens the Format dialog; `formatWhy` says why not, when not.
  // `onreplace`/`replaceWhy` and `onfirmware`/`firmwareWhy` the same for
  // Replace… and Update firmware… (#317).
  import { groupActions } from '../driveactions.js'

  let { actions = [], onformat = null, formatWhy = '', onreplace = null, replaceWhy = '', onfirmware = null, firmwareWhy = '', run, label = 'Actions', count = null } = $props()

  let open = $state(false)
  let at = $state({ top: 0, left: 0, width: 260, up: false, max: 400 })
  let button
  let menu = $state(null)

  const groups = $derived(
    groupActions(actions, {
      format: onformat ? { enabled: !formatWhy, why: formatWhy } : null,
      replace: onreplace ? { enabled: !replaceWhy, why: replaceWhy } : null,
      firmware: onfirmware ? { enabled: !firmwareWhy, why: firmwareWhy } : null,
    }),
  )

  function toggle(e) {
    e.stopPropagation()
    if (open) return (open = false)
    place()
    open = true
  }
  // Where the menu goes, from where its button is now: again when the page
  // scrolls or reflows under it (a live page grows above it), not closed.
  function place() {
    const r = button.getBoundingClientRect()
    const width = Math.min(280, window.innerWidth - 16)
    const left = Math.max(8, Math.min(r.right - width, window.innerWidth - width - 8))
    // Below the button, or above it when the window has more room there.
    const below = window.innerHeight - r.bottom - 12
    const above = r.top - 12
    const up = below < 260 && above > below
    at = up ? { top: window.innerHeight - r.top + 4, left, width, up, max: above } : { top: r.bottom + 4, left, width, up, max: below }
  }
  $effect(() => {
    if (!open) return
    const close = (e) => {
      if (e?.type === 'click' && e.target instanceof Node && (menu?.contains(e.target) || button?.contains(e.target))) return
      open = false
    }
    const follow = (e) => {
      if (e.target instanceof Node && menu?.contains(e.target)) return
      const r = button?.getBoundingClientRect()
      if (!r || r.bottom < 0 || r.top > window.innerHeight) open = false
      else place()
    }
    const key = (e) => e.key === 'Escape' && close()
    window.addEventListener('click', close)
    window.addEventListener('scroll', follow, true)
    window.addEventListener('resize', follow)
    window.addEventListener('keydown', key)
    return () => {
      window.removeEventListener('click', close)
      window.removeEventListener('scroll', follow, true)
      window.removeEventListener('resize', follow)
      window.removeEventListener('keydown', key)
    }
  })

  function pick(item) {
    open = false
    if (item.format) onformat?.()
    else if (item.replace) onreplace?.()
    else if (item.firmware) onfirmware?.()
    else run?.(item.action)
  }
</script>

<span class="wrap">
  <button bind:this={button} class="trigger" aria-haspopup="menu" aria-expanded={open} onclick={toggle} data-testid="drive-actions">
    {label} <span aria-hidden="true">▾</span>
  </button>
  {#if open}
    <div class="menu" role="menu" style="{at.up ? 'bottom' : 'top'}: {at.top}px; left: {at.left}px; width: {at.width}px; max-height: {at.max}px" bind:this={menu}>
      {#each groups as g (g.name)}
        <div class="group" class:danger={g.danger}>
          <div class="gname">{g.name}{#if g.danger}<span class="mark"> · destroys data</span>{/if}</div>
          {#each g.items as item (item.key)}
            <button role="menuitem" class:danger={item.danger} disabled={!item.enabled} title={item.why || ''} onclick={() => pick(item)}>
              {item.label}{#if item.danger}<span class="warnmark" aria-label="destructive"> ⚠</span>{/if}
              {#if count !== null && item.count !== undefined && item.count !== count}<span class="dim"> · {item.count} of {count}</span>{/if}
              {#if !item.enabled && item.why}<span class="why">{item.why}</span>{/if}
            </button>
          {/each}
        </div>
      {:else}
        <p class="none">No actions.</p>
      {/each}
    </div>
  {/if}
</span>

<style>
  .wrap { display: inline-block; }
  .trigger { font-size: var(--sc-t-meta); padding: 4px 11px; white-space: nowrap; }
  .menu {
    position: fixed;
    z-index: 80;
    overflow-y: auto;
    display: grid;
    gap: 2px;
    padding: 4px;
    background: var(--panel);
    border: 1px solid var(--border-strong);
    border-radius: var(--radius);
    box-shadow: 0 6px 20px rgb(0 0 0 / 0.28);
  }
  .group + .group { border-top: 1px solid var(--border); padding-top: 3px; margin-top: 2px; }
  .gname { font-size: var(--sc-t-eyebrow); text-transform: uppercase; letter-spacing: 0.05em; color: var(--text-faint); padding: 4px 10px 2px; }
  .group.danger .gname { color: var(--error); }
  .mark { text-transform: none; letter-spacing: 0; }
  .menu button {
    display: block;
    width: 100%;
    text-align: left;
    background: none;
    border: none;
    border-radius: var(--radius-sm);
    padding: 6px 10px;
    font-size: var(--sc-t-body);
    color: var(--text);
    white-space: normal;
  }
  .menu button:hover:not(:disabled) { background: var(--nav-hover); }
  .menu button:disabled { color: var(--text-ghost); }
  .menu button.danger { color: var(--error); }
  .menu button.danger:disabled { color: color-mix(in srgb, var(--error) 45%, transparent); }
  .menu button.danger:hover:not(:disabled) { background: color-mix(in srgb, var(--error) 16%, transparent); }
  .why { display: block; font-size: var(--sc-t-meta); color: var(--text-dim); }
  .dim { color: var(--text-dim); font-size: var(--sc-t-meta); }
  .none { margin: 6px 10px; font-size: var(--sc-t-meta); color: var(--text-dim); }
</style>
