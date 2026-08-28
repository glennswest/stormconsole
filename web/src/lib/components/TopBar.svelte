<script>
  import { auth, feed, nav, logout } from '../stores.svelte.js'
  import { THEMES, theme, applyTheme } from 'stormview/theme'
</script>

<header>
  <a class="brand" href="#/">⛈ {nav.name}</a>
  <span class="right">
    <select
      class="theme-pick"
      title="Theme"
      value={theme.current}
      onchange={(e) => applyTheme(e.target.value)}
    >
      {#each THEMES as t}
        <option value={t.id}>{t.label}</option>
      {/each}
    </select>
    <span class="live" class:on={feed.connected} title={feed.connected ? 'live' : 'reconnecting'}></span>
    {#if auth.required}
      {#if auth.user}<span class="user">{auth.user}</span>{/if}
      <button class="signout" title="Sign out" onclick={logout}>⏻</button>
    {/if}
  </span>
</header>

<style>
  header {
    grid-area: top;
    background: var(--panel);
    border-bottom: 1px solid var(--border);
    padding: 0 20px;
    display: flex;
    align-items: center;
    height: var(--nav-h, 48px);
    gap: 8px;
  }
  .brand {
    font-size: 17px;
    font-weight: 700;
    color: var(--brand);
    letter-spacing: -0.5px;
    white-space: nowrap;
  }
  .brand:hover { text-decoration: none; filter: brightness(1.15); }
  .right {
    margin-left: auto;
    font-size: 12px;
    color: var(--text-dim);
    display: flex;
    align-items: center;
    gap: 8px;
    white-space: nowrap;
  }
  .live {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: var(--text-ghost);
    transition: background 0.3s;
  }
  .live.on { background: var(--ok); box-shadow: 0 0 6px var(--ok); }
  .theme-pick {
    padding: 3px 6px;
    font-size: 12px;
    background: var(--panel-raised);
    color: var(--text-dim);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
  }
  .user { color: var(--accent); font-weight: 600; }
  .signout {
    padding: 2px 8px;
    font-size: 13px;
    background: none;
    border: 1px solid var(--border);
    color: var(--text-dim);
  }
  .signout:hover { color: var(--error); border-color: var(--error-border); }
</style>
