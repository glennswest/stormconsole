// What the Audit page makes of a record (#15), apart from the page so plain node can test it.

export const CAPABILITIES = {
  console: 'Opened a console',
  'sign-in': 'Sign-in',
  storage: 'Destructive storage',
  delete: 'Delete',
  image: 'Registry image',
  power: 'Machine power',
  cluster: 'Cluster membership',
  update: 'Update',
  change: 'Other change',
}

export const capability = (c) => CAPABILITIES[c] || c

/// How it ended. A refusal is worth seeing as one.
export function result(status) {
  if (status === 101 || (status >= 200 && status < 300)) return { word: 'done', tone: 'ok' }
  if (status === 401 || status === 403 || status === 428) return { word: `refused (${status})`, tone: 'warn' }
  return { word: `failed (${status})`, tone: 'bad' }
}

/// A console a viewer opens shows and takes no typing, so the record says which it was.
export function what(r) {
  if (r.capability === 'console' && r.may_write === false) return `${r.object} (watching only)`
  return r.object
}
