// The console's chrome styles (#151). Orthogonal to stormview's themes: a
// theme is the palette, a style is the proportion and structure. Both
// work on all twelve palettes, so picking one never constrains the other.
//
// Pure, so node can test it: `node web/src/lib/ui/styles.test.mjs`.

export const STYLES = [
  { id: 'containerfocus', label: 'ContainerFocus' },
  { id: 'esxi', label: 'ESXi' },
]

export const DEFAULT_STYLE = 'containerfocus'

// Ids a style used to have. A choice stored under an old id opens in the
// style it became, so a rename never loses anyone's setting.
const RENAMED = { openshift: 'containerfocus' }

/// The style a stored (or chosen) id means: itself when known, what it
/// was renamed to, else the default.
export function styleId(id) {
  const v = RENAMED[id] ?? id
  return STYLES.some((s) => s.id === v) ? v : DEFAULT_STYLE
}
