// A stormd that must not be read, on the node page (#49).
const { walk } = require('./lib.cjs')
const { NODE, STRANGE } = process.env

walk('stormd-tls', async (t) => {
  const page = await t.page()
  t.check(await t.open(page, `#/node/${NODE}`, /was not read/), 'the node page says a port was not read', (await t.text(page)).slice(0, 400))
  const text = await t.text(page)
  t.check(new RegExp(`:${STRANGE} was not read`).test(text), 'which one')
  t.check(/UnknownIssuer|certificate/i.test(text), 'and why', (text.match(/.{0,40}was not read.{0,120}/) || [''])[0])
  await t.shot(page, 'node-refused')
})
