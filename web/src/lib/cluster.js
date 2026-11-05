// What the Cluster page takes from stormcluster's feed (#161). Pure, so a
// node test can check it (cluster.test.mjs).

const metric = (c, label) => c?.metrics?.find((m) => m.label === label)?.value ?? ''

/// A node's pod network. Its release's flavour is what it is running: a
/// flowsdn release is `<n>-flowsdn`, the only other flavour is cilium. Only
/// with no release is what it announced the answer.
export function networkOf(card) {
  const release = metric(card, 'release')
  if (release) return release.endsWith('-flowsdn') ? 'flowsdn' : 'cilium'
  return metric(card, 'network')
}

/// The machines a form or a join may offer: a single node in no other
/// cluster, stormcluster running there, heard lately, not a laptop, not this
/// one. Nothing else on the segment is this cluster's to take.
export function freeNodes(peers, self) {
  return (peers || []).filter(
    (p) => p.kind === 'peer' && p.detail === 'single-node cluster, available' && p.health !== 'error' && p.label !== self
  )
}

/// The cluster's own card, with nothing about the segment on it.
export function clusterCard(system) {
  if (!system) return system
  return {
    ...system,
    detail: (system.detail || '').replace(/; \d+ peer\(s\) discovered$/, ''),
    metrics: (system.metrics || []).filter((m) => m.label !== 'peers'),
  }
}
