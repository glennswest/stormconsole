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

/// The version a container runs: the node's word on its image first, then stormcentral's
/// record of it.
export function versionOf(c) {
  return c?.imageBuild?.version || c?.golden?.version || ''
}

/// A container's image as the row leads with it. `null` when nothing names one.
export function imageLine(c) {
  const named = c?.clonedFrom ? fromName(c.clonedFrom) : null
  const component = c?.golden?.component || named?.repository
  if (!component) return null
  const tag = versionOf(c) || named?.tag || fromName(c?.golden?.name).tag || (c?.golden?.commit || '').slice(0, 12)
  const ref = tag ? `registry/${component}:${tag}` : `registry/${component}`
  return { ref, digest: withAlgo(c?.golden?.deviceSha256), version: versionOf(c) }
}
