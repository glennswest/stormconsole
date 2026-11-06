// What the Images page says about a registry's credential (#35), over deploy/verify-registry-auth.sh.
const { walk } = require('./lib.cjs')
const { WANT, NOT } = process.env

walk('registry credential', async (t) => {
  const page = await t.page()
  t.check(await t.open(page, '#/images', new RegExp(WANT), 25000), `the Images page reads /${WANT}/`, (await t.text(page)).slice(0, 400))
  if (NOT) t.check(!new RegExp(NOT).test(await t.text(page)), `and not /${NOT}/`, (await t.text(page)).slice(0, 400))
  await t.shot(page, `images-${WANT.replace(/\W+/g, '-').slice(0, 30)}`)
})
