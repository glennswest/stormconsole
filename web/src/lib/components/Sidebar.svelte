<script>
  // The navigator, rendered entirely from the server's merged nav feed —
  // a new plugin appears here with no frontend change. Groups collapse,
  // the active item carries an accent rail, and every countable route
  // shows how many objects it holds, so the tree answers "is anything
  // there?" before you click it.
  import { nav, navCount, prefs, toggleSection } from '../stores.svelte.js'
  import { iconFor } from '../ui/icons.js'
  import Icon from './Icon.svelte'

  let current = $state(location.hash || '#/')
  window.addEventListener('hashchange', () => (current = location.hash || '#/'))

  const isActive = (href) => current === href || (href !== '#/' && current.startsWith(href))
</script>

<aside class:collapsed={!prefs.navOpen}>
  <nav aria-label="Console navigation">
    {#each nav.sections as section (section.label)}
      {@const shut = !!prefs.collapsed[section.label]}
      <div class="section">
        <button
          class="section-label"
          aria-expanded={!shut}
          onclick={() => toggleSection(section.label)}
        >
          <span class="caret" class:shut><Icon name="down" size={12} stroke={2} /></span>
          {section.label}
        </button>
        {#if !shut}
          <ul>
            {#each section.items as item (item.href)}
              {@const n = navCount(item.href)}
              <li>
                <a href={item.href} class:active={isActive(item.href)} aria-current={isActive(item.href) ? 'page' : undefined}>
                  <Icon name={iconFor(item.label, item.href)} size={15} />
                  <span class="label">{item.label}</span>
                  {#if n !== null}<span class="count" class:zero={n === 0}>{n}</span>{/if}
                </a>
              </li>
            {/each}
          </ul>
        {/if}
      </div>
    {/each}
  </nav>
</aside>

<style>
  aside {
    grid-area: side;
    background: var(--panel);
    border-right: 1px solid var(--border);
    padding: 10px 0 24px;
    overflow-y: auto;
    overflow-x: hidden;
  }
  aside.collapsed { display: none; }

  .section { margin-bottom: 4px; }

  .section-label {
    display: flex;
    align-items: center;
    gap: 4px;
    width: 100%;
    background: none;
    border: none;
    border-radius: 0;
    padding: calc(var(--sc-nav-py) + 2px) 12px 4px;
    font-size: var(--sc-t-eyebrow);
    font-weight: 700;
    text-transform: uppercase;
    letter-spacing: 0.07em;
    color: var(--text-faint);
    text-align: left;
  }
  .section-label:hover { background: none; color: var(--text-dim); }
  .caret { display: grid; place-items: center; transition: transform 0.15s ease; }
  .caret.shut { transform: rotate(-90deg); }

  ul { list-style: none; }

  a {
    display: flex;
    align-items: center;
    gap: 9px;
    padding: var(--sc-nav-py) 12px var(--sc-nav-py) 13px;
    font-size: var(--sc-nav-font);
    color: var(--text-dim);
    /* The rail is always present and usually transparent, so the label
       never shifts when an item becomes active. */
    border-left: var(--sc-nav-rail) solid transparent;
  }
  a:hover { color: var(--text); background: var(--nav-hover); text-decoration: none; }
  a.active {
    color: var(--text);
    background: var(--nav-active);
    border-left-color: var(--accent);
    font-weight: 600;
  }
  a :global(svg) { color: var(--text-ghost); }
  a:hover :global(svg) { color: var(--text-faint); }
  a.active :global(svg) { color: var(--accent); }

  .label { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .count {
    margin-left: auto;
    font-size: var(--sc-t-eyebrow);
    font-variant-numeric: tabular-nums;
    color: var(--text-faint);
    background: var(--panel-raised);
    border-radius: 999px;
    padding: 0 6px;
    min-width: 20px;
    text-align: center;
  }
  .count.zero { opacity: 0.45; }
  a.active .count { color: var(--text); }
</style>
