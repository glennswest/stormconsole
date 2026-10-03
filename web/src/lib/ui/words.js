// What the console calls things, where that differs from the API (#70).
//
// The API's word for a sealed image on forge is "golden"; a person reads
// "registry image" — nothing runs on one, a pod or VM runs on its
// instance, a copy-on-write clone of it. The token stays in ids, kinds,
// relation and metric names (automation reads them); this turns it into
// the page's word wherever the SPA shows one. Exact tokens only: a name
// such as `golden-stormlb` is data and is never rewritten. Mirrors
// `console_core::words::term`.
const TERMS = {
  golden: 'registry image',
  goldens: 'registry images',
  slab_golden: 'slab registry image',
  slab_goldens: 'slab registry images',
}

export function term(token) {
  if (typeof token !== 'string') return token
  return TERMS[token] ?? TERMS[token.toLowerCase()] ?? token
}

// A component as a page shows it: kind, metric labels, the `kind`
// metric's value and relation names in the page's words. For components
// rendered by stormview's card, which prints those fields as they come.
export function shown(c) {
  if (!c) return c
  return {
    ...c,
    kind: term(c.kind),
    metrics: (c.metrics || []).map((m) => ({
      ...m,
      label: term(m.label),
      value: m.label === 'kind' ? term(m.value) : m.value,
    })),
    relations: (c.relations || []).map((r) => ({ ...r, name: term(r.name) })),
  }
}

// Does a relation answer to this name in either vocabulary? A card's
// chip links with the word it shows, an older link with the API's.
export function sameRelation(name, wanted) {
  return name === wanted || term(name) === term(wanted)
}
