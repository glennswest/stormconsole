<script>
  // Every volume a container is on, from both ends (#347): its own root (a copy of its
  // image), its data and logs volumes and its claims. Found on this node's engine by
  // the ids the node writes on the pod (`storm.io/instance.<c>`, `storm.io/volume.<name>`)
  // and by the owner the engine records (Pod ns/name, container c), so a volume either
  // side names is listed. Each links to its own row, which names this pod back.
  import { get } from '../api.js'
  import { formatBytes } from '../drivemap.js'
  import { registryName } from '../imagename.js'
  import { term } from '../ui/words.js'

  let { ns, pod, container, volumes = [], node = '', mirror = false } = $props()

  let answer = $state(null)
  let error = $state('')
  const ids = $derived((container.volumeIds || []).join(','))
  const asking = $derived(
    new URLSearchParams({ owner: 'Pod', namespace: ns, name: pod, container: container.name, ids }).toString()
  )
  // The page reloads its pod every few seconds; this asks once per change of what it names.
  $effect(() => {
    get(`/api/plugins/sb/runs-on?${asking}`)
      .then((a) => (answer = a))
      .catch((e) => (error = e.message))
  })

  const b = (n) => (n == null ? '—' : formatBytes(n))
  // The pod volume and mount a listed engine volume is, by the id the node wrote.
  function mountedAs(id) {
    if (id === container.instance) return [{ as: 'root', at: 'its own copy of the image' }]
    const out = []
    for (const v of volumes.filter((v) => v.engineId === id)) {
      const ms = (v.mounts || []).filter((m) => m.container === container.name)
      if (!ms.length) out.push({ as: v.name, at: v.claim ? `claim ${v.claim.name}` : '' })
      for (const m of ms) out.push({ as: v.name, at: `${m.mountPath}${m.readOnly ? ' (read-only)' : ''}`, claim: v.claim?.name })
    }
    return out
  }
  // Whose writer has not recorded an owner yet, so the gap names the right issue.
  function ownerGap(id) {
    if (mirror) return 'stormd#75'
    if (volumes.some((v) => v.engineId === id && v.claim)) return 'stormblock-csi#69'
    return 'rustkube-node#372'
  }
  function ownerHref(o) {
    if (!o?.link) return ''
    const m = o.link.match(/^k8s:pod:([^/]+)\/(.+)$/)
    return m ? `#/pod/${m[1]}/${m[2]}` : `#/grid?id=${encodeURIComponent(o.link)}`
  }
  // This container's mounts the node names no engine volume for: a claim on another
  // driver, or one the node has not named yet.
  const unnamed = $derived(
    volumes.filter(
      (v) => v.claim && !v.engineId && (v.mounts || []).some((m) => m.container === container.name)
    )
  )
</script>

{#if error}
  <p class="none">{error.includes('404') ? 'This console reads no engine.' : `The engine did not answer: ${error}`}</p>
{:else if answer?.error}
  <p class="none">{answer.error}</p>
{:else if answer}
  {#if answer.volumes.length}
    <div class="table-wrap">
      <table>
        <thead><tr><th>Mounted as</th><th>Volume</th><th>What</th><th>Cloned from</th><th>Written</th><th>Health</th><th>Owner</th></tr></thead>
        <tbody>
          {#each answer.volumes as v (v.runsOn.id)}
            <tr>
              <td>
                {#each mountedAs(v.runsOn.id) as m}<div><span class="mono">{m.as}</span>{#if m.at} <span class="dim">{m.at}</span>{/if}</div>{:else}<span class="dim">not mounted by this container</span>{/each}
              </td>
              <td>
                <a class="mono" href={`#/grid?id=${encodeURIComponent(`sb:volume:${v.runsOn.id}`)}`}>{v.runsOn.name || v.runsOn.id}</a>
                <div class="dim mono small">{v.runsOn.id}</div>
              </td>
              <td>{term(v.kind)}{#if v.attached?.length}<div class="dim small">{v.attached.join(', ')}</div>{/if}</td>
              <td>
                {#if v.clonedFrom}
                  <span class="mono" title={v.clonedFrom.name}>{registryName(v.clonedFrom.name) || v.clonedFrom.name}</span>
                  <div class="dim mono small">{#if v.clonedFrom.digest}sha256:{v.clonedFrom.digest.replace(/^sha256:/, '').slice(0, 16)}…{:else}no digest recorded{/if}</div>
                {:else}<span class="dim">made empty</span>{/if}
              </td>
              <td>{b(v.runsOn.allocatedBytes)} <span class="dim">of {b(v.runsOn.virtualBytes)}</span></td>
              <td class:ok={v.runsOn.health === 'healthy'} class:warn={v.runsOn.health && v.runsOn.health !== 'healthy'}>{v.runsOn.health || '—'}</td>
              <td>
                {#if v.owner}
                  {#if ownerHref(v.owner)}<a href={ownerHref(v.owner)}>{v.owner.words}</a>{:else}{v.owner.words}{/if}
                {:else}<span class="dim">not recorded ({ownerGap(v.runsOn.id)})</span>{/if}
              </td>
            </tr>
          {/each}
        </tbody>
      </table>
    </div>
  {:else if container.volumeIds?.length}
    <p class="none">This node's engine holds none of the volumes the node names ({container.volumeIds.join(', ')}).{#if node} It runs on {node}: that node's console can say.{/if}</p>
  {:else}
    <p class="none">The node names no volume for this container, and the engine records none as its ({mirror ? 'stormd#75' : 'rustkube-node#372'}).</p>
  {/if}
  {#each unnamed as v (v.name)}
    <p class="dim small">{v.name}: claim {v.claim.name}{#if v.claim.pv?.driver} on {v.claim.pv.driver}{/if} — the node names no engine volume for it.</p>
  {/each}
{:else}
  <p class="none">Reading the engine…</p>
{/if}

<style>
  .table-wrap { overflow-x: auto; }
  table { border-collapse: collapse; width: 100%; }
  th, td { text-align: left; padding: 4px 10px 4px 0; vertical-align: top; border-bottom: 1px solid var(--border); }
  th { color: var(--text-faint); font-weight: normal; }
  .dim { color: var(--text-dim); }
  .small { font-size: var(--sc-t-meta); }
  .mono { font-family: var(--mono); font-size: var(--sc-t-meta); }
  .ok { color: var(--ok); }
  .warn { color: var(--warn); }
  .none { color: var(--text-dim); margin: 0; }
</style>
