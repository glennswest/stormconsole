<script>
  // The OpenShift-style left nav, rendered entirely from the server's
  // merged nav feed — a new plugin appears here with no frontend change.
  import { nav } from '../stores.svelte.js'

  let current = $state(location.hash || '#/')
  window.addEventListener('hashchange', () => (current = location.hash))
</script>

<aside>
  {#each nav.sections as section (section.label)}
    <div class="section">
      <div class="section-label">{section.label}</div>
      {#each section.items as item (item.href)}
        <a href={item.href} class:active={current === item.href}>{item.label}</a>
      {/each}
    </div>
  {/each}
</aside>

<style>
  aside {
    grid-area: side;
    background: var(--panel);
    border-right: 1px solid var(--border);
    padding: 12px 8px;
    overflow-y: auto;
  }
  .section { margin-bottom: 14px; }
  .section-label {
    font-size: 11px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.5px;
    color: var(--text-faint);
    padding: 4px 12px;
  }
  a {
    display: block;
    padding: 6px 12px;
    border-radius: var(--radius-sm);
    font-size: 13px;
    color: var(--text-dim);
  }
  a:hover { color: var(--text); background: var(--nav-hover); text-decoration: none; }
  a.active { color: var(--text); background: var(--nav-active); }
</style>
