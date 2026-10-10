// The Machines page on a node that does not run stormipmi (#60): it says
// "not started on this node", not an error.
const { walk } = require('./lib.cjs')

walk('not started', async (t) => {
  const page = await t.page()
  t.check(await t.open(page, '#/machines', /not started on this node/), 'the Machines page says stormipmi is not started here', (await t.text(page)).slice(0, 400))
  const text = await t.text(page)
  t.check(/roles=sno/.test(text) && /boot\.d/.test(text), 'with the role it runs on and how to start it')
  t.check(!/did not answer/.test(text), 'and no error')
  await t.shot(page, 'machines-not-started')
})
