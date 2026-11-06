<script>
  // Network Policies and Network Endpoints (#168). The owner: "Network Policies (No
  // cillium) Network Endpoints (No Cillium), and these should come from either cillium or
  // flowsdn". One table whichever network this is; the source is the small "from:" line.
  //
  // In the project picked in the masthead, or every project with none picked.
  import { onDestroy } from 'svelte'
  import { get } from '../api.js'
  import { route } from '../router.svelte.js'
  import { build, k8sns } from '../stores.svelte.js'
  import { mergeEndpoints, inProject, search, rulesLine, enforcement, selectsLine, fromLine } from '../network.js'
  import PageHeader from '../components/PageHeader.svelte'
  import EmptyState from '../components/EmptyState.svelte'

  const policies = route.current.name === 'networkpolicies'
  let rows = $state(null)
  let network = $state('')
  let scope = $state('')
  let error = $state('')
  let needle = $state('')
  let picked = $state('')
  let timer = null

  async function load() {
    try {
      const objects = await get(`/api/plugins/k8s/network/${policies ? 'policies' : 'endpoints'}`)
      network = objects.network
      if (policies) {
        rows = objects.items
      } else {
        // Only a flowsdn node has an agent to ask.
        const agent = build.network === 'flowsdn' ? await get('/api/plugins/flowsdn/network-endpoints').catch(() => null) : null
        rows = mergeEndpoints(objects.items, agent?.items)
        network = network || agent?.network || ''
        scope = agent?.items?.length ? agent.scope : ''
      }
      error = ''
    } catch (e) {
      error = `the console did not answer: ${e.message}`
    }
    timer = setTimeout(load, 8000)
  }
  load()
  onDestroy(() => clearTimeout(timer))

  // As every list reads it: the link's own project first, else the masthead's (#5).
  const project = $derived(route.current.query.has('ns') ? route.current.query.get('ns') : k8sns.selected || '')
  const shown = $derived(search(inProject(rows || [], project), needle))
  const title = policies ? 'Network Policies' : 'Network Endpoints'
</script>

<div class="sc-page">
  <PageHeader
    crumbs={project ? [{ label: 'Projects', href: '#/projects' }, { label: project, href: `#/k8s/ns/${encodeURIComponent(project)}` }, { label: title }] : [{ label: 'Networking' }, { label: title }]}
    {title}
    scope={project || 'every project'}
    count={rows ? shown.length : null}
  />

  {#if error}<p class="bad">{error}</p>{/if}
  {#if rows}
    <div class="bar">
      <input bind:value={needle} placeholder={policies ? 'A name, a selector' : 'A name, an address, a node, a state'} aria-label="Search" />
      {#if fromLine(shown, network)}<span class="from">{fromLine(shown, network)}</span>{/if}
    </div>
    {#if scope}<p class="dim small">{scope}</p>{/if}

    {#if !shown.length}
      <EmptyState
        icon="policy"
        title={policies ? 'No network policies' : 'No network endpoints'}
        hint={rows.length
          ? 'Nothing matches here. Pick another project, or clear the search.'
          : policies
            ? (network ? 'Nothing restricts traffic: every workload can reach every other.' : 'No policy exists, and no pod network has its objects on this cluster.')
            : (network ? 'The pod network has no endpoints yet.' : 'No pod network on this cluster reports endpoints.')}
      />
    {:else if policies}
      <div class="table-wrap">
        <table>
          <thead><tr><th>Name</th><th>Project</th><th>Sort</th><th>Selects</th><th>Ingress</th><th>Egress</th><th>Applies to</th></tr></thead>
          <tbody>
            {#each shown as p (p.id)}
              <tr>
                <td><a class="mono" href={`#/grid?id=${encodeURIComponent(p.id)}`}>{p.name}</a></td>
                <td>{p.project || 'cluster-wide'}</td>
                <td class="small dim">{p.sort}</td>
                <td class="mono small">{selectsLine(p)}{#if p.unevaluated}<div class="warn nomono">uses expressions, so what it applies to is not worked out here</div>{/if}</td>
                <td class="small">{rulesLine(p, 'ingress')}</td>
                <td class="small">{rulesLine(p, 'egress')}</td>
                <td>
                  <button class="link" aria-expanded={picked === p.id} onclick={() => (picked = picked === p.id ? '' : p.id)}>{p.appliesTo.length + p.withheld} {p.appliesTo.length + p.withheld === 1 ? 'pod' : 'pods'}</button>
                  {#if picked === p.id}
                    <ul class="plain small">
                      {#each p.appliesTo as a (a.project + a.name)}<li><a class="mono" href={a.href}>{a.project}/{a.name}</a></li>{/each}
                      {#if p.withheld}<li class="dim">{p.withheld} in projects you cannot see</li>{/if}
                    </ul>
                  {/if}
                </td>
              </tr>
            {/each}
          </tbody>
        </table>
      </div>
    {:else}
      <div class="table-wrap">
        <table>
          <thead><tr><th>Workload</th><th>Project</th><th>Addresses</th><th>Node</th><th>Identity</th><th>Enforced in</th><th>Enforced out</th><th>State</th></tr></thead>
          <tbody>
            {#each shown as e (e.id)}
              <tr>
                <td><span class="tag">{e.kind}</span><a class="mono gap" href={e.href}>{e.name}</a></td>
                <td>{e.project}</td>
                <td class="mono small">{#each e.ips as ip (ip)}<div>{ip}</div>{:else}—{/each}</td>
                <td class="mono small">{e.node || '—'}</td>
                <td class="small"><span class="mono">{e.identity ?? '—'}</span>{#if e.identityLabels}<div class="dim">{e.identityLabels}</div>{/if}</td>
                <td class="small" class:good={e.ingress === true}>{enforcement(e.ingress)}</td>
                <td class="small" class:good={e.egress === true}>{enforcement(e.egress)}</td>
                <td class="small" class:good={e.ready} class:warn={!e.ready}>{e.state || 'not said'}</td>
              </tr>
            {/each}
          </tbody>
        </table>
      </div>
    {/if}
  {/if}
</div>

<style>
  .small { font-size: var(--sc-t-meta); }
  .dim { color: var(--text-dim); }
  .warn { color: var(--warn-strong); }
  .good { color: var(--ok); }
  .bad { color: var(--error); }
  .mono { font-family: var(--mono); font-size: var(--sc-t-meta); }
  .nomono { font-family: inherit; }
  .gap { margin-left: 8px; }
  .bar { display: flex; gap: 12px; align-items: center; flex-wrap: wrap; margin-bottom: 12px; }
  .bar input { width: 300px; max-width: 100%; }
  .from { font-size: var(--sc-t-meta); color: var(--text-faint); margin-left: auto; }
  .tag { font-size: var(--sc-t-eyebrow); padding: 1px 6px; border-radius: 999px; border: 1px solid var(--border); color: var(--text-dim); }
  .plain { list-style: none; margin: 4px 0 0; padding: 0; }
  button.link { background: none; border: 0; padding: 0; color: var(--accent); cursor: pointer; font: inherit; }
  .table-wrap { overflow-x: auto; }
  table { width: 100%; border-collapse: collapse; background: var(--panel); border: 1px solid var(--border); border-radius: var(--radius); font-size: var(--sc-t-body); }
  th { text-align: left; font-size: var(--sc-t-eyebrow); text-transform: uppercase; letter-spacing: 0.05em; color: var(--text-faint); padding: 8px 12px; border-bottom: 1px solid var(--border); }
  td { padding: 6px 12px; vertical-align: top; overflow-wrap: anywhere; }
  tr + tr > td { border-top: 1px solid var(--sc-hairline); }
</style>
