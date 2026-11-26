// Which node's ironprom a chart is read from (#369). Each node's ironprom holds only its own series, so a page
// for an object on another node names that node, and the console asks the ironprom there.

/** The charts query for an object, with its node when the page knows it. */
export function chartQuery(asked, node) {
  const p = new URLSearchParams(asked)
  if (node && !p.get('on')) p.set('on', node)
  return p
}

/** Which node a Metrics page's charts are from: the one named, or a port's or RAID set's own node (#385). '' is this node. */
export function chartNode(query) {
  const p = new URLSearchParams(query)
  return p.get('on') || (['port', 'raidset'].includes(p.get('entity')) ? p.get('node') || '' : '')
}

/** A Metrics level's page for a node (#386): `on` names it, and '' is this node's. */
export function levelHref(level, node) {
  const p = new URLSearchParams({ level })
  if (node) p.set('on', node)
  return `#/metrics?${p}`
}

/** Where a chart's series came from, in the page's words. */
export function fromWords(node) {
  return node ? `From ${node}’s ironprom` : 'From this node’s ironprom'
}
