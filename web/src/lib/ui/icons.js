// A 16px stroked icon set for the navigator and page headers.
//
// The nav feed is assembled by the server from whatever plugins are
// mounted, so icons are matched to an item by its route and label rather
// than declared — a new plugin gets a sensible glyph without a frontend
// change, and `dot` is the honest fallback when nothing matches.

export const ICONS = {
  overview: 'M3 3h7v7H3zM14 3h7v7h-7zM3 14h7v7H3zM14 14h7v7h-7z',
  node: 'M3 5h18v5H3zM3 14h18v5H3zM7 7.5h.01M7 16.5h.01',
  pod: 'M12 2 3 7v10l9 5 9-5V7zM3 7l9 5 9-5M12 12v10',
  workload: 'M4 4h7v7H4zM13 4h7v7h-7zM4 13h7v7H4zM13 13h7v7h-7z',
  network: 'M12 3v6M5 21a2 2 0 1 0 0-4 2 2 0 0 0 0 4ZM19 21a2 2 0 1 0 0-4 2 2 0 0 0 0 4ZM12 11a2 2 0 1 0 0-4 2 2 0 0 0 0 4ZM5 17v-2a2 2 0 0 1 2-2h10a2 2 0 0 1 2 2v2',
  storage: 'M4 6c0-1.7 3.6-3 8-3s8 1.3 8 3-3.6 3-8 3-8-1.3-8-3ZM4 6v12c0 1.7 3.6 3 8 3s8-1.3 8-3V6M4 12c0 1.7 3.6 3 8 3s8-1.3 8-3',
  image: 'm12 2 9 5-9 5-9-5zM3 12l9 5 9-5M3 17l9 5 9-5',
  logs: 'M4 6h16M4 11h16M4 16h10M4 21h6',
  events: 'M18 8a6 6 0 1 0-12 0c0 7-3 9-3 9h18s-3-2-3-9M13.7 21a2 2 0 0 1-3.4 0',
  cluster: 'M12 2a10 10 0 1 0 0 20 10 10 0 0 0 0-20ZM2 12h20M12 2c2.5 2.7 3.9 6.3 4 10-.1 3.7-1.5 7.3-4 10-2.5-2.7-3.9-6.3-4-10 .1-3.7 1.5-7.3 4-10Z',
  policy: 'M12 3 4 6v6c0 4.6 3.4 8.5 8 9 4.6-.5 8-4.4 8-9V6ZM9 12l2 2 4-4',
  service: 'M12 3v4M12 17v4M3 12h4M17 12h4M12 15a3 3 0 1 0 0-6 3 3 0 0 0 0 6Z',
  search: 'M11 19a8 8 0 1 0 0-16 8 8 0 0 0 0 16ZM21 21l-4.3-4.3',
  table: 'M3 5h18v14H3zM3 10h18M9 10v9',
  cards: 'M3 4h8v7H3zM13 4h8v7h-8zM3 13h8v7H3zM13 13h8v7h-8z',
  refresh: 'M21 12a9 9 0 1 1-2.6-6.4M21 4v5h-5',
  chevron: 'm9 6 6 6-6 6',
  down: 'm6 9 6 6 6-6',
  dot: 'M12 13a1 1 0 1 0 0-2 1 1 0 0 0 0 2Z',
  power: 'M12 3v9M18.4 6.6a9 9 0 1 1-12.7 0',
  plus: 'M12 5v14M5 12h14',
  inbox: 'M3 12h5l2 3h4l2-3h5M5 5h14l2 7v7H3v-7Z',
  filter: 'M3 5h18l-7 8v6l-4 2v-8Z',
}

const RULES = [
  [/events/i, 'events'],
  [/log/i, 'logs'],
  [/overview|home|dashboard/i, 'overview'],
  [/node|host|fleet|machine/i, 'node'],
  [/pod|container/i, 'pod'],
  [/deploy|statefulset|daemonset|job|workload|replica/i, 'workload'],
  [/polic/i, 'policy'],
  [/service|route|ingress|endpoint/i, 'service'],
  [/network|cilium|cni|identit/i, 'network'],
  [/volume|slab|array|disk|drive|storage|pvc|export|lun/i, 'storage'],
  [/image|golden|clone|pallet|registry/i, 'image'],
  [/namespace|project|cluster/i, 'cluster'],
]

/// The glyph for a nav item: matched on its label first, then its route.
export function iconFor(label = '', href = '') {
  const hay = `${label} ${href}`
  for (const [re, name] of RULES) if (re.test(hay)) return name
  return 'dot'
}
