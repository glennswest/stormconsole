// The agent's counters on the pod network's card (#4).
const { walk } = require('./lib.cjs')

walk('cilium agent metrics', async (t) => {
  const page = await t.page()
  t.check(await t.open(page, '#/', /Pod network/i), 'the card is on the overview', (await t.text(page)).slice(0, 300))
  const more = page.getByRole('button', { name: /Show all/ }).first()
  if (await more.count()) await more.click()
  t.check(await t.until(async () => /denied by policy/i.test(await t.text(page))), 'with what policy denied', (await t.text(page)).slice(0, 500))
  const text = await t.text(page)
  t.check(/dropped/i.test(text) && /forwarded/i.test(text), 'what it dropped and forwarded')
  t.check(/cilium_ct4_global 93%/.test(text), 'and the fullest map')
  await t.shot(page, 'cilium-card')
})
