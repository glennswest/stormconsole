// Admin → Users (#266), apart from the page so plain node can test it.

/// The roles worth offering first: what stormcos ships for storage, then
/// the kubernetes ones a console role follows from.
export const SUGGESTED = ['storage-admin', 'storage-viewer', 'cluster-admin', 'edit', 'view']

/// What a ClusterRole lets a person do on this console. The apiserver
/// decides every request made as them; this is what the console offers.
export function means(role) {
  switch (role) {
    case 'cluster-admin':
      return 'administrator: every action, every page'
    case 'storage-admin':
      return 'format drives and RAID sets, as themselves'
    case 'operator':
    case 'edit':
      return 'changes things'
    default:
      return 'reads'
  }
}

/// The offered roles, the suggested ones first and only those the cluster has.
export function offer(roles) {
  const have = new Set(roles || [])
  const first = SUGGESTED.filter((r) => have.has(r))
  const rest = [...have].filter((r) => !SUGGESTED.includes(r)).sort()
  return [...first, ...rest]
}

/// A name the apiserver will take for a ServiceAccount; '' if it will.
export function nameProblem(name) {
  if (!name) return 'a name is required'
  if (name.length > 63) return 'at most 63 characters'
  if (!/^[a-z0-9]([-a-z0-9]*[a-z0-9])?$/.test(name)) return "lower-case letters, digits and '-', not at either end"
  return ''
}

/// What is wrong with a new user before it is sent; '' when nothing.
export function newUserProblem({ name, password, again, roles }) {
  return (
    nameProblem(name) ||
    (!password ? 'a password is required' : '') ||
    (password !== again ? 'the passwords differ' : '') ||
    (!roles || !roles.length ? 'give the user a role' : '')
  )
}
