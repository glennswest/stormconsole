<script>
  // One kubernetes kind as a grid over the feed, scoped by the namespace
  // selector. Rows are components; nesting and actions come from their
  // relations, so this view holds no model of its own.
  import { route } from '../router.svelte.js'
  import { feed, k8sns } from '../stores.svelte.js'
  import ComponentGrid from 'stormview/components/ComponentGrid.svelte'
  import { call } from '../api.js'
  import CreateMenu from '../components/CreateMenu.svelte'

  const titles = {
    pod: 'Pods',
    deploy: 'Deployments',
    sts: 'StatefulSets',
    ds: 'DaemonSets',
    job: 'Jobs',
    cronjob: 'CronJobs',
    svc: 'Services',
    pvc: 'PersistentVolumeClaims',
    node: 'Nodes',
    ns: 'Namespaces',
  }
  const namespaced = ['pod', 'deploy', 'sts', 'ds', 'job', 'cronjob', 'svc', 'pvc']

  const kind = $derived(route.current.params.kind)
  const title = $derived(titles[kind] || kind)

  const rootIds = $derived.by(() => {
    const prefix = `k8s:${kind}:`
    let ids = feed.components.filter((c) => c.id.startsWith(prefix)).map((c) => c.id)
    if (k8sns.selected && namespaced.includes(kind)) {
      ids = ids.filter((id) => id.startsWith(`${prefix}${k8sns.selected}/`))
    }
    return ids.sort()
  })
</script>

<div class="content">
  <h1>
    {title}
    {#if k8sns.selected && namespaced.includes(kind)}
      <span class="scope">in {k8sns.selected}</span>
    {/if}
    <span class="count">{rootIds.length}</span>
    <span class="tools"><CreateMenu at={`#/k8s/${kind}`} /></span>
  </h1>
  {#if !feed.loaded}
    <div class="empty">Connecting…</div>
  {:else if rootIds.length === 0}
    <div class="empty">
      No {title.toLowerCase()}{k8sns.selected ? ` in ${k8sns.selected}` : ''}.
      <div class="empty-create"><CreateMenu at={`#/k8s/${kind}`} primary={true} /></div>
    </div>
  {:else}
    <ComponentGrid components={feed.components} {rootIds} invoke={(a) => call(a.method, a.path)} />
  {/if}
</div>

<style>
  h1 {
    display: flex;
    align-items: baseline;
    gap: 10px;
    font-size: 16px;
    font-weight: 600;
    margin-bottom: 14px;
  }
  .scope { font-size: 13px; color: var(--text-dim); font-weight: 400; }
  .count {
    font-size: 12px;
    color: var(--text-faint);
    background: var(--panel-raised);
    border: 1px solid var(--border);
    border-radius: 10px;
    padding: 1px 8px;
  }
  .empty { color: var(--text-faint); padding: 40px; text-align: center; }
  .empty-create { margin-top: 12px; }
  .tools { margin-left: auto; }
</style>
