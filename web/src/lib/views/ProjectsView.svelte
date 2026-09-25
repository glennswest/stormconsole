<script>
  // Projects: the top of the console (#28). The viewer's own — the ones the
  // apiserver says they are in — each with who asked for it and whether it
  // is fenced off, and a New project form. System namespaces are not here;
  // they are the Cluster section's.
  import { route } from '../router.svelte.js'
  import { projects, loadProjects, newProject, selectNamespace, noteActivity } from '../stores.svelte.js'
  import PageHeader from '../components/PageHeader.svelte'
  import EmptyState from '../components/EmptyState.svelte'

  let form = $state({ name: '', displayName: '', description: '' })
  let busy = $state(false)
  let error = $state('')
  let asking = $state(route.current.query.get('new') === '1')

  $effect(() => {
    loadProjects()
  })
  $effect(() => {
    if (asking && !form.name && projects.suggested && !projects.list.some((p) => p.name === projects.suggested)) {
      form.name = projects.suggested
    }
  })

  async function create() {
    busy = true
    error = ''
    try {
      const name = await newProject(form.name.trim(), form.displayName, form.description)
      noteActivity({ reason: 'New project', message: `project ${name} created`, source: 'Projects' })
      selectNamespace(name)
      location.hash = `#/k8s/ns/${encodeURIComponent(name)}`
    } catch (e) {
      error = e.message
    }
    busy = false
  }
</script>

<div class="sc-page">
  <PageHeader crumbs={[{ label: 'Projects' }]} title="Projects" count={projects.loaded ? projects.list.length : null}>
    {#snippet actions()}
      {#if projects.write && !asking}<button class="sc-primary" onclick={() => (asking = true)}>New project</button>{/if}
    {/snippet}
  </PageHeader>

  <p class="lead">
    A project is a namespace of your own: your machines, pods and claims go in one, never beside the
    system's objects. You are its admin, and you decide who else is in it.
    {#if !projects.served}This apiserver serves no projects, so these are its namespaces.{/if}
  </p>

  {#if error || projects.error}<p class="error">{error || projects.error}</p>{/if}

  {#if asking}
    <section class="card">
      <h2>New project</h2>
      <div class="form">
        <input bind:value={form.name} placeholder="name — lowercase, digits, '-'" aria-label="Name" />
        <input bind:value={form.displayName} placeholder="display name (optional)" aria-label="Display name" />
        <input bind:value={form.description} placeholder="description (optional)" aria-label="Description" />
        <div class="row">
          <button class="sc-primary" disabled={busy || !form.name.trim()} onclick={create}>Create project</button>
          <button disabled={busy} onclick={() => (asking = false)}>Cancel</button>
        </div>
      </div>
    </section>
  {/if}

  {#if projects.loaded && !projects.list.length && !asking}
    <EmptyState icon="cluster" title="No projects yet"
      hint="Create one for your machines and pods — every create in the console asks which project it goes in.">
      {#snippet action()}
        {#if projects.write}<button class="sc-primary" onclick={() => (asking = true)}>New project</button>{/if}
      {/snippet}
    </EmptyState>
  {:else if projects.list.length}
    <table class="projects">
      <thead><tr><th>Project</th><th>Requested by</th><th>Status</th><th></th></tr></thead>
      <tbody>
        {#each projects.list as p (p.name)}
          <tr>
            <td>
              <a href={`#/k8s/ns/${encodeURIComponent(p.name)}`}>{p.name}</a>
              {#if p.displayName}<span class="dim"> · {p.displayName}</span>{/if}
              {#if p.description}<div class="dim">{p.description}</div>{/if}
            </td>
            <td>{p.requester || '—'}</td>
            <td>{p.phase || '—'}</td>
            <td>{#if p.isolated}<span class="badge" title={p.dns ? 'isolated, DNS allowed' : 'isolated, no DNS'}>isolated</span>{/if}</td>
          </tr>
        {/each}
      </tbody>
    </table>
  {/if}
</div>

<style>
  .lead { font-size: var(--sc-t-body); color: var(--text-dim); max-width: 75ch; margin: 0 0 14px; }
  .error { color: var(--error); font-size: var(--sc-t-body); }
  .card { background: var(--panel); border: 1px solid var(--border); border-radius: var(--radius); padding: 14px var(--sc-row-px); margin-bottom: 12px; }
  h2 { font-size: var(--sc-t-eyebrow); text-transform: uppercase; letter-spacing: 0.06em; color: var(--text-faint); margin: 0 0 10px; }
  .form { display: grid; gap: 8px; max-width: 520px; }
  .row { display: flex; gap: 8px; }
  table.projects { width: 100%; border-collapse: collapse; background: var(--panel); border: 1px solid var(--border); border-radius: var(--radius); }
  table.projects th { text-align: left; font-size: var(--sc-t-eyebrow); text-transform: uppercase; letter-spacing: 0.05em; color: var(--text-faint); padding: 8px 12px; border-bottom: 1px solid var(--border); }
  table.projects td { padding: 8px 12px; font-size: var(--sc-t-body); vertical-align: top; }
  table.projects tr + tr > td { border-top: 1px solid var(--sc-hairline); }
  .dim { color: var(--text-dim); font-size: var(--sc-t-meta); }
  .badge { font-size: var(--sc-t-eyebrow); text-transform: uppercase; letter-spacing: 0.05em; padding: 1px 6px; border-radius: var(--radius-sm); border: 1px solid var(--accent); color: var(--accent); }
</style>
