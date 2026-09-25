<script>
  // Attach a claim that is waiting for its first consumer (#28). A claim
  // whose class binds on first use is provisioned when something mounts it;
  // this is the something, one click per machine in the claim's project.
  // A pod's volumes are fixed when it is created, so for a pod the answer is
  // to name the claim in its spec — said, not offered as a button.
  import { route } from '../router.svelte.js'
  import { feed, noteActivity } from '../stores.svelte.js'
  import { postJson } from '../api.js'
  import PageHeader from '../components/PageHeader.svelte'

  const ns = $derived(route.current.params.ns)
  const claim = $derived(route.current.params.name)
  let busy = $state('')
  let error = $state('')
  let saved = $state('')

  const machines = $derived(
    feed.components.filter((c) => c.id.startsWith(`vm:machine:${ns}/`)).sort((a, b) => a.label.localeCompare(b.label))
  )

  async function attach(m) {
    busy = m.id
    error = ''
    saved = ''
    try {
      const r = await postJson(
        `/api/plugins/vm/vms/${encodeURIComponent(ns)}/${encodeURIComponent(m.label)}/disks`,
        { name: claim, source: 'pvc', from: claim }
      )
      saved = r.message
      noteActivity({ reason: 'Attach claim', message: saved, source: `PersistentVolumeClaim/${claim}` })
    } catch (e) {
      error = e.message
    }
    busy = ''
  }
</script>

<div class="sc-page">
  <PageHeader
    crumbs={[{ label: 'Storage' }, { label: 'PVCs', href: '#/k8s/pvc' }, { label: claim }]}
    title={`Attach ${claim}`}
    scope={`in ${ns}`}
  />
  <p class="lead">
    {claim} is Pending because its storage class provisions on first use: nothing is allocated until a
    pod or VM mounts it. Give it to a machine in {ns}, and it is provisioned when that machine next starts.
    For a pod, name <span class="mono">{claim}</span> under its <span class="mono">volumes</span>.
  </p>
  {#if error}<p class="error">{error}</p>{/if}
  {#if saved}<p class="saved">{saved}</p>{/if}
  {#if !machines.length}
    <p class="dim">No virtual machines with a definition in {ns}.</p>
  {:else}
    <table class="vms">
      <tbody>
        {#each machines as m (m.id)}
          <tr>
            <td><a href={m.link}>{m.label}</a></td>
            <td class="dim">{m.detail}</td>
            <td class="acts"><button disabled={!!busy} onclick={() => attach(m)}>Attach as a disk</button></td>
          </tr>
        {/each}
      </tbody>
    </table>
  {/if}
</div>

<style>
  .lead { font-size: var(--sc-t-body); color: var(--text-dim); max-width: 75ch; margin: 0 0 14px; }
  .mono { font-family: var(--mono); font-size: var(--sc-t-meta); }
  .error { color: var(--error); }
  .saved { color: var(--ok); }
  .dim { color: var(--text-dim); font-size: var(--sc-t-meta); }
  table.vms { width: 100%; border-collapse: collapse; background: var(--panel); border: 1px solid var(--border); border-radius: var(--radius); }
  table.vms td { padding: 8px 12px; font-size: var(--sc-t-body); }
  table.vms tr + tr > td { border-top: 1px solid var(--sc-hairline); }
  .acts { text-align: right; }
</style>
