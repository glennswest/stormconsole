// How the console names a component's image (#85, stormcos docs/GLOSSARY.md): the user's
// form is `registry/<component>:<version>` plus its digest. "The sha is the manifest … a
// version number is part of meta data. And every pod must always show the version its
// running" (owner, #79). Apart from the pages so plain node can test it.

const plain = (n) => n.replace(/-(raw|sealed)$/, '').replace(/^fstemplate-/, '')

/// The repository and tag a volume's name says: `golden-<component>-<tag>` or
/// `<component>-<tag>`, the tag a build id (12 hex) or a content sha (16 hex).
/// Mirrors `holdings::from_name`.
export function fromName(name) {
  const bare = plain(name || '').replace(/^golden-/, '')
  const at = bare.lastIndexOf('-')
  if (at > 0) {
    const tag = bare.slice(at + 1)
    if ((tag.length === 12 || tag.length === 16) && /^[0-9a-fA-F]+$/.test(tag)) return { repository: bare.slice(0, at), tag }
  }
  return { repository: bare, tag: '' }
}

/// The user's form of an image's name, or '' when the name is not a component's image.
export function registryName(name) {
  const { repository, tag } = fromName(name)
  return tag ? `registry/${repository}:${tag}` : ''
}

const withAlgo = (d) => (!d ? '' : d.includes(':') ? d : `sha256:${d}`)

/// The version a container runs: the node's word on its image (`storm.io/image-build.<c>`).
export function versionOf(c) {
  return c?.imageBuild?.version || ''
}

/// A container's image as the row leads with it, from the node's own objects (#347): the
/// registry image it was cloned from (`storm.io/golden.<c>`), its version and the digest
/// the node recorded (`storm.io/image-digest.<c>`). `null` when the node names none.
export function imageLine(c) {
  const named = c?.clonedFrom ? fromName(c.clonedFrom) : null
  if (!named?.repository) return null
  const tag = versionOf(c) || named.tag
  const ref = tag ? `registry/${named.repository}:${tag}` : `registry/${named.repository}`
  return { ref, digest: withAlgo(c?.digest), version: versionOf(c) }
}

/// The image's build as the node read it from its OCI config: version, revision (the
/// commit), created, source — the keys it carries, in that order, then any others.
export function buildLine(c) {
  const b = c?.imageBuild
  if (!b || typeof b !== 'object') return ''
  const short = (k) => k.replace(/^org\.opencontainers\.image\./, '')
  const order = ['version', 'revision', 'created', 'source']
  const entries = Object.entries(b)
    .filter(([, v]) => v !== null && v !== '' && typeof v !== 'object')
    .map(([k, v]) => [short(k), String(v)])
    .sort(([a], [z]) => (order.indexOf(a) < 0 ? 99 : order.indexOf(a)) - (order.indexOf(z) < 0 ? 99 : order.indexOf(z)))
  return entries.map(([k, v]) => `${k} ${k === 'revision' ? v.slice(0, 12) : v}`).join(' · ')
}
