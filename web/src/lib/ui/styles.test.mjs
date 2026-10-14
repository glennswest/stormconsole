// The style ids (#151), with plain node: `node web/src/lib/ui/styles.test.mjs`.
import assert from 'node:assert/strict'
import { STYLES, DEFAULT_STYLE, styleId } from './styles.js'

assert.equal(DEFAULT_STYLE, 'containerfocus')
assert.ok(STYLES.some((s) => s.id === DEFAULT_STYLE), 'the default is a style')
assert.equal(STYLES.find((s) => s.id === 'containerfocus').label, 'ContainerFocus')
// A choice stored before the rename opens in the renamed style.
assert.equal(styleId('openshift'), 'containerfocus')
// Known ids are themselves; anything else is the default.
for (const s of STYLES) assert.equal(styleId(s.id), s.id)
assert.equal(styleId(undefined), DEFAULT_STYLE)
assert.equal(styleId('nonsense'), DEFAULT_STYLE)
// No style is labelled with another company's product name.
for (const s of STYLES) assert.ok(!/openshift/i.test(s.label + s.id), s.label)
console.log('styles: PASS')
