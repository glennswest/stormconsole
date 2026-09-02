<script>
  // The masthead. Identity on the left, the scope you are working in
  // beside it, and the controls that apply everywhere on the right —
  // create, cluster health, appearance, session.
  import {
    auth, feed, nav, logout, k8sns, selectNamespace, prefs, rollup,
    STYLES, applyStyle,
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
      class="pick"
      aria-label="Console style"
      title="Console style — layout and density"
      value={prefs.style}
      onchange={(e) => applyStyle(e.target.value)}
    >
      {#each STYLES as s}
        <option value={s.id}>{s.label}</option>
      {/each}
    </select>

    <select
      class="pick"
      aria-label="Theme"
      title="Theme — colours"
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
    border-bottom: 1px solid var(--sc-masthead-line);
    padding: 0 12px 0 8px;
    display: flex;
    align-items: center;
    height: var(--nav-h);
    gap: 10px;
    position: relative;
    z-index: 20;
    /* Both styles put a dark bar at the top, whatever the palette below
       it, so the masthead carries its own foreground rather than the
       theme's — otherwise a light theme paints dark text on dark. */
    color: var(--sc-masthead-fg);
    color-scheme: dark;
  }

  .hamburger {
    background: none;
    border: none;
    color: var(--sc-masthead-dim);
    padding: 6px;
    display: grid;
    place-items: center;
  }
  .hamburger:hover { background: rgb(255 255 255 / 0.1); color: var(--sc-masthead-fg); }

  .brand {
    display: inline-flex;
    align-items: center;
    gap: 9px;
    padding: 4px 6px;
    border-radius: var(--radius-sm);
    color: var(--sc-masthead-fg);
    white-space: nowrap;
  }
  .brand:hover { text-decoration: none; background: rgb(255 255 255 / 0.1); }
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
    border-left: 1px solid var(--sc-masthead-line);
    height: 26px;
  }
  .scope label {
    font-size: var(--sc-t-eyebrow);
    text-transform: uppercase;
    letter-spacing: 0.06em;
    color: var(--sc-masthead-dim);
  }
  .scope select { max-width: 220px; }

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
    border: 1px solid var(--sc-masthead-line);
    border-radius: 999px;
    background: rgb(255 255 255 / 0.07);
  }
  .health:hover { text-decoration: none; background: rgb(255 255 255 / 0.14); }
  /* The palette's state colours are tuned for the content ground; on a
     near-black bar the light-theme greens and reds go muddy, so lift them
     toward white for the masthead only. */
  .health :global(.sc-status.ok) { color: color-mix(in srgb, var(--ok) 55%, white); }
  .health :global(.sc-status.warn) { color: color-mix(in srgb, var(--warn-strong) 60%, white); }
  .health :global(.sc-status.error) { color: color-mix(in srgb, var(--error) 62%, white); }
  .health :global(.sc-status.unknown) { color: var(--sc-masthead-dim); }
  .health :global(.mark) { background: rgb(255 255 255 / 0.1); }
  .live {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--text-ghost);
    transition: background 0.3s;
  }
  .live.on { background: var(--ok); box-shadow: 0 0 5px var(--ok); }

  /* One control treatment for every select on the bar. */
  header :global(select) {
    padding: 3px 22px 3px 8px;
    font-size: var(--sc-t-meta);
    color: var(--sc-masthead-fg);
    background: rgb(255 255 255 / 0.08);
    border: 1px solid var(--sc-masthead-line);
    border-radius: var(--radius-sm);
  }
  header :global(select:hover) { background: rgb(255 255 255 / 0.14); }

  .user { font-size: var(--sc-t-meta); color: var(--sc-masthead-dim); font-weight: 500; }
  .signout {
    padding: 4px 8px;
    background: none;
    border: 1px solid transparent;
    color: var(--sc-masthead-dim);
    display: grid;
    place-items: center;
  }
  .signout:hover {
    color: #fff;
    border-color: var(--error);
    background: color-mix(in srgb, var(--error) 55%, transparent);
  }

  @media (max-width: 900px) {
    .scope label, .user { display: none; }
    .scope { padding-left: 10px; margin-left: 2px; }
  }
  @media (max-width: 700px) {
    .word { display: none; }
    .pick { display: none; }
  }
</style>
