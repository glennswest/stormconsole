<script>
  // The masthead. Identity on the left, the scope you are working in
  // beside it, and the controls that apply everywhere on the right —
  // create, cluster health, appearance, session.
  import {
    auth, feed, nav, logout, k8sns, selectNamespace, prefs, rollup,
  } from '../stores.svelte.js'
  import { THEMES, theme, applyTheme } from 'stormview/theme'
  import CreateMenu from './CreateMenu.svelte'
  import Icon from './Icon.svelte'
  import StatusPill from './StatusPill.svelte'

  let namespaces = $derived(
    feed.components
      .filter((c) => c.kind === 'k8s-ns')
      .map((c) => c.label)
      .sort()
  )

  const health = $derived(rollup())
  const worst = $derived(health.error ? 'error' : health.warn ? 'warn' : health.total ? 'ok' : 'unknown')
  const healthText = $derived(
    !feed.loaded
      ? 'Connecting'
      : health.error
        ? `${health.error} failed`
        : health.warn
          ? `${health.warn} degraded`
          : health.total
            ? `${health.total} healthy`
            : 'No components'
  )
</script>

<header>
  <button
    class="hamburger"
    title="Toggle navigation"
    aria-label="Toggle navigation"
    aria-expanded={prefs.navOpen}
    onclick={() => (prefs.navOpen = !prefs.navOpen)}
  >
    <Icon name="logs" size={18} />
  </button>

  <a class="brand" href="#/">
    <span class="mark" aria-hidden="true">
      <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor"
           stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
        <path d="M7 15a4 4 0 0 1 .5-8 5.5 5.5 0 0 1 10.4 1.6A3.7 3.7 0 0 1 17.5 15" />
        <path d="m12 11-2.5 5H13l-1.5 5" />
      </svg>
    </span>
    <span class="word">{nav.name}</span>
  </a>

  {#if namespaces.length}
    <span class="scope">
      <label for="ns-pick">Namespace</label>
      <select
        id="ns-pick"
        value={k8sns.selected}
        onchange={(e) => selectNamespace(e.target.value)}
      >
        <option value="">All namespaces</option>
        {#each namespaces as ns}
          <option value={ns}>{ns}</option>
        {/each}
      </select>
    </span>
  {/if}

  <span class="right">
    <CreateMenu at={null} label="Create" primary={true} />

    <a class="health" href="#/" title="Cluster health — {health.ok} ready, {health.warn} degraded, {health.error} failed">
      <StatusPill health={worst} label={healthText} />
      <span class="live" class:on={feed.connected} title={feed.connected ? 'Live' : 'Reconnecting…'}></span>
    </a>

    <select
      class="theme-pick"
      aria-label="Appearance"
      title="Appearance"
      value={theme.current}
      onchange={(e) => applyTheme(e.target.value)}
    >
      {#each THEMES as t}
        <option value={t.id}>{t.label}</option>
      {/each}
    </select>

    {#if auth.required}
      {#if auth.user}<span class="user">{auth.user}</span>{/if}
      <button class="signout" title="Sign out" aria-label="Sign out" onclick={logout}>
        <Icon name="power" size={15} />
      </button>
    {/if}
  </span>
</header>

<style>
  header {
    grid-area: top;
    background: var(--sc-masthead);
    border-bottom: 1px solid var(--border);
    padding: 0 12px 0 8px;
    display: flex;
    align-items: center;
    height: var(--nav-h);
    gap: 10px;
    position: relative;
    z-index: 20;
  }

  .hamburger {
    background: none;
    border: none;
    color: var(--text-dim);
    padding: 6px;
    display: grid;
    place-items: center;
  }
  .hamburger:hover { background: var(--nav-hover); color: var(--text); }

  .brand {
    display: inline-flex;
    align-items: center;
    gap: 9px;
    padding: 4px 6px;
    border-radius: var(--radius-sm);
    color: var(--text);
    white-space: nowrap;
  }
  .brand:hover { text-decoration: none; background: var(--nav-hover); }
  .mark { color: var(--brand); display: grid; place-items: center; }
  .word { font-size: 15px; font-weight: 600; letter-spacing: -0.01em; }

  /* The working scope, set off by a rule the way OpenShift sets off its
     project selector from the brand. */
  .scope {
    display: inline-flex;
    align-items: center;
    gap: 8px;
    margin-left: 8px;
    padding-left: 16px;
    border-left: 1px solid var(--border);
    height: 26px;
  }
  .scope label {
    font-size: var(--sc-t-eyebrow);
    text-transform: uppercase;
    letter-spacing: 0.06em;
    color: var(--text-faint);
  }
  .scope select {
    font-size: var(--sc-t-meta);
    padding: 3px 22px 3px 8px;
    max-width: 220px;
    background: var(--panel);
  }

  .right {
    margin-left: auto;
    display: flex;
    align-items: center;
    gap: 10px;
    white-space: nowrap;
  }

  .health {
    display: inline-flex;
    align-items: center;
    gap: 8px;
    padding: 4px 10px;
    border: 1px solid var(--border);
    border-radius: 999px;
    background: var(--panel);
  }
  .health:hover { text-decoration: none; border-color: var(--border-strong); }
  .live {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--text-ghost);
    transition: background 0.3s;
  }
  .live.on { background: var(--ok); box-shadow: 0 0 5px var(--ok); }

  .theme-pick {
    padding: 3px 22px 3px 8px;
    font-size: var(--sc-t-meta);
    color: var(--text-dim);
    background: var(--panel);
  }

  .user { font-size: var(--sc-t-meta); color: var(--text-dim); font-weight: 500; }
  .signout {
    padding: 4px 8px;
    background: none;
    border: 1px solid transparent;
    color: var(--text-dim);
    display: grid;
    place-items: center;
  }
  .signout:hover { color: var(--error); border-color: var(--error-border); background: var(--error-bg); }

  @media (max-width: 900px) {
    .scope label, .user { display: none; }
    .scope { padding-left: 10px; margin-left: 2px; }
  }
  @media (max-width: 700px) {
    .word, .theme-pick { display: none; }
  }
</style>
