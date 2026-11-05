// What the Registry page makes of the holdings (#163), apart from the page so plain
// node can test it. The words are OCI Distribution's: repository, tag, digest.

export const HOLDERS = [
  ['boot-host', 'Boot hosts', 'Boot host'],
  ['pod', 'Pods', 'Pod'],
  ['claim', 'Claims', 'Claim'],
  ['vm', 'VMs', 'VM'],
  ['service', 'Services', 'Service'],
  ['release-disk', 'Release disks', 'Release disk'],
  ['build', 'Builds', 'Build'],
  ['attached', 'Attached', 'Attached'],
  ['other', 'Other', 'Other'],
  ['nobody', 'Nobody', 'Nobody'],
]

export const holderWord = (kind) => HOLDERS.find((h) => h[0] === kind)?.[2] || kind

/// What to call who holds it.
export function holderName(h) {
  if (h.withheld) return 'in a project you cannot see'
  if (h.kind === 'nobody') return 'nobody'
  return h.namespace ? `${h.namespace}/${h.name}` : h.name
}

/// A catalog's or engine's kind, in the page's words.
export function imageKind(k) {
  return (
    {
      component: 'component',
      golden: 'container image',
      blank: 'blank',
      media: 'VM media',
      base: 'base',
      slab_golden: 'slab member',
      release_part: 'release part',
      template: 'template',
      snapshot: 'snapshot',
      sealed: 'sealed',
    }[k] || k || 'sealed'
  )
}

export const ref = (i) => (i.tag ? `${i.repository}:${i.tag}` : i.repository)

export function shortDigest(d) {
  if (!d) return ''
  const [algo, hex] = d.includes(':') ? d.split(':') : ['', d]
  return `${algo ? `${algo}:` : ''}${hex.slice(0, 12)}`
}

/// Every clone with its image beside it: the reverse view's rows, and the counts.
export function clonesOf(holdings) {
  const out = []
  for (const r of holdings?.repositories || []) {
    for (const i of r.images) for (const c of i.clones) out.push({ ...c, image: i })
  }
  for (const c of holdings?.unparented || []) out.push({ ...c, image: null })
  return out
}

function matches(c, needle) {
  if (!needle) return true
  return [c.name, c.holder.kind, c.holder.name, c.holder.namespace, c.holder.detail, c.image ? ref(c.image) : '', c.image?.name]
    .join(' ')
    .toLowerCase()
    .includes(needle)
}

/// The holders with counts, for the filter: only kinds there are any of.
export function holderCounts(holdings) {
  const by = holdings?.by_holder || {}
  return HOLDERS.filter(([k]) => by[k]).map(([k, label]) => ({ kind: k, label, count: by[k] }))
}

/// Repositories narrowed: a holder kind or a search keeps an image that has a
/// matching clone; a search on the image's own name keeps all its clones.
export function narrowed(holdings, { search = '', kind = '', repo = '' } = {}) {
  const needle = search.toLowerCase()
  const out = []
  for (const r of holdings?.repositories || []) {
    if (repo && r.name !== repo) continue
    const images = []
    for (const i of r.images) {
      const named = !needle || `${ref(i)} ${i.name} ${i.digest}`.toLowerCase().includes(needle)
      const clones = i.clones.filter((c) => (!kind || c.holder.kind === kind) && (named || matches({ ...c, image: i }, needle)))
      if (kind ? clones.length : named || clones.length) images.push({ ...i, clones, all: i.clones.length })
    }
    if (images.length) out.push({ ...r, images })
  }
  return out
}

/// The reverse view: pick a machine or a pod, see which tag it runs on.
export function byHolder(holdings, { search = '', kind = '' } = {}) {
  const needle = search.toLowerCase()
  return clonesOf(holdings)
    .filter((c) => (!kind || c.holder.kind === kind) && matches(c, needle))
    .sort((a, b) => a.holder.kind.localeCompare(b.holder.kind) || holderName(a.holder).localeCompare(holderName(b.holder)))
}

export function bytes(n) {
  if (n == null) return '—'
  const u = ['B', 'KiB', 'MiB', 'GiB', 'TiB', 'PiB']
  let v = n
  let i = 0
  while (v >= 1024 && i < u.length - 1) {
    v /= 1024
    i++
  }
  return `${i ? v.toFixed(1) : v} ${u[i]}`
}
