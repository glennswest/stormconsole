<script>
  // What a container's environment comes to (#12): the references resolved, as far as this person may read
  // them. A Secret's value is never here.
  import { get } from '../api.js'
  let { base, containers = [] } = $props()
  const names = containers.map((c) => c.name)
  let container = $state(names[0] || '')
  let data = $state(null)
  let error = $state('')
  $effect(() => {
    const c = container
    if (!c) return
    get(`${base}/env?container=${encodeURIComponent(c)}`).then(
      (d) => { data = d; error = '' },
      (e) => { error = e.message },
    )
  })
</script>

<section class="card">
  <h2>Environment
    {#if names.length > 1}
      <select bind:value={container} aria-label="Container">
        {#each names as n (n)}<option value={n}>{n}</option>{/each}
      </select>
    {:else}<span class="dim">— {container}</span>{/if}
  </h2>
  <p class="dim small">What the spec comes to. The image’s own variables and the Service variables the node adds are not listed: <code>env</code> in the Terminal shows everything.</p>
  {#if error}
    <p class="bad">{error}</p>
  {:else if data && !data.vars.length}
    <p class="dim">None set in the spec.</p>
  {:else if data}
    <table class="env">
      <thead><tr><th>Name</th><th>Value</th><th>From</th></tr></thead>
      <tbody>
        {#each data.vars as v (v.name)}
          <tr>
            <td class="mono">{v.name}</td>
            <td class="mono value">
              {#if v.hidden}<span class="dim">value not shown</span>{:else if v.value !== null}{v.value}{:else}<span class="dim">—</span>{/if}
              {#if v.problem}<div class="warn small">{v.problem}</div>{/if}
            </td>
            <td class="dim">{v.source}</td>
          </tr>
        {/each}
      </tbody>
    </table>
  {/if}
</section>

<style>
  h2 { font-size: var(--sc-t-eyebrow); text-transform: uppercase; letter-spacing: 0.06em; color: var(--text-faint); margin: 0 0 10px; }
  .card { background: var(--panel); border: 1px solid var(--border); border-radius: var(--radius); padding: 14px var(--sc-row-px); }
  h2 select { margin-left: 8px; font-size: var(--sc-t-body); font-weight: 400; text-transform: none; letter-spacing: 0; }
  table.env { width: 100%; border-collapse: collapse; font-size: var(--sc-t-body); }
  table.env th { text-align: left; color: var(--text-faint); font-size: var(--sc-t-meta); font-weight: 600; padding: 6px 10px 6px 0; border-bottom: 1px solid var(--border); }
  table.env td { padding: 6px 10px 6px 0; border-bottom: 1px solid var(--sc-hairline); vertical-align: top; }
  .value { overflow-wrap: anywhere; }
  .mono { font-family: var(--mono, monospace); }
  .dim { color: var(--text-dim); }
  .small { font-size: var(--sc-t-meta); }
  .warn { color: var(--warning, var(--error)); font-family: inherit; }
  .bad { color: var(--error); }
</style>
