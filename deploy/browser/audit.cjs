// The Audit page in a browser (#15), over deploy/verify-audit.sh's acts.
const { walk } = require('./lib.cjs')

walk('audit', async (t) => {
  const page = await t.page('root')
  t.check(await t.open(page, '#/audit', /mallory/), 'the page opens with what was done', (await t.text(page)).slice(0, 300))
  let text = await t.text(page)
  t.check(/Opened a console/.test(text), 'a console, in words')
  t.check(/refused \(403\)/.test(text) && /partly refused \(207\)/.test(text) && /done/.test(text), 'allowed and refused, each said')
  const nav = await page.evaluate(() => fetch('/api/v1/console/nav').then((r) => r.json()))
  t.check(JSON.stringify(nav).includes('#/audit'), 'in the navigator')
  await t.shot(page, 'audit')
  await page.locator('input').first().fill('mallory')
  t.check(await t.until(async () => !/k8s\/apply/.test(await t.text(page))), 'narrowed by who')
  await page.locator('input').first().fill('')
  await page.locator('select').last().selectOption('delete')
  t.check(await t.until(async () => /configmaps\/c1/.test(await t.text(page)) && !/mallory/.test(await t.text(page))), 'narrowed by what')
  await t.shot(page, 'audit-deletes')

  const ops = await t.page('ops')
  t.check(await t.open(ops, '#/audit', /administrator/), "an operator is told whose it is")
})
