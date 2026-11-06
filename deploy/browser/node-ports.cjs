// stormcluster and stormimds's stormd on the node page (#68).
const { walk } = require('./lib.cjs')
const { NODE, MODE } = process.env

walk('node-ports', async (t) => {
  const page = await t.page()
  if (MODE === 'read') {
    t.check(await t.open(page, `#/node/${NODE}`, /8269/), 'the node page lists stormimds\'s stormd', (await t.text(page)).slice(0, 400))
    const text = await t.text(page)
    t.check(/9102/.test(text), 'and stormcluster\'s feed', text.slice(0, 400))
    t.check(!/:9102 was not read/.test(text), 'read, not refused')
    t.check(!/Quiet:[^\n]*\b9102\b/.test(text), 'and not quiet')
    t.check(!/Quiet:[^\n]*\b8269\b/.test(text), '8269 is not quiet')
    await t.shot(page, 'node-ports')
  } else {
    t.check(await t.open(page, `#/node/${NODE}`, /:9102 was not read/), 'with no certificates the page says :9102 was not read', (await t.text(page)).slice(0, 400))
    t.check(/TLS only/.test(await t.text(page)), 'in stormcluster\'s words')
    await t.shot(page, 'node-ports-refused')
  }
})
