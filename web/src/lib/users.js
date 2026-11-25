// Admin → Users (#266), apart from the page so plain node can test it.

/// The roles worth offering first: what stormcos ships for storage and
/// hardware (#391), then the kubernetes ones a console role follows from.
export const SUGGESTED = ['storage-admin', 'storage-viewer', 'hardware-admin', 'cluster-admin', 'edit', 'view']

/// What a ClusterRole lets a person do on this console. The apiserver
/// decides every request made as them; this is what the console offers.
export function means(role) {
  switch (role) {
    case 'cluster-admin':
      return 'administrator: every action, every page'
    case 'storage-admin':
      return 'format drives and RAID sets, as themselves'
    case 'hardware-admin':
      return 'power machines, repoint what they boot, type on their consoles'
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

/// What is wrong with a new user before it is sent; '' when nothing. A
/// group can stand in for a role (#334).
export function newUserProblem({ name, password, again, roles, groups }) {
  return (
    nameProblem(name) ||
    (!password ? 'a password is required' : '') ||
    (password !== again ? 'the passwords differ' : '') ||
    (!roles?.length && !groups?.length ? 'give the user a role or a group' : '')
  )
}

/// A group's name: a user's rule, less room for `-group` (#334).
export function groupNameProblem(name) {
  if (name && name.length > 57) return 'at most 57 characters'
  return nameProblem(name)
}

/// The roles a user holds through their groups and not in their own right,
/// each with the groups it comes from.
export function viaGroups(user, groups) {
  const out = new Map()
  for (const g of groups || []) {
    if (!(g.members || []).includes(user.name)) continue
    for (const r of g.roles || []) {
      if ((user.own || []).includes(r)) continue
      out.set(r, [...(out.get(r) || []), g.name])
    }
  }
  return [...out].map(([role, from]) => ({ role, from })).sort((a, b) => a.role.localeCompare(b.role))
}

/// Who may be put in a group: every user but stormcos's own admin.
export const memberChoices = (users) => (users || []).filter((u) => !u.builtin).map((u) => u.name)
