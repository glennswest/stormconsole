// A node's rack on its page (#77), over deploy/verify-node-rack.sh. PHASE: hand-root, hand-reader, hand-bob, owned.
const { walk } = require('./lib.cjs')
const { PHASE, HOST } = process.env
const WHO = { 'hand-root': 'root', 'hand-reader': 'reader', 'hand-bob': 'bob', owned: 'root' }[PHASE]

walk(`node rack (${PHASE})`, async (t) => {
  const page = await t.page(WHO)
  t.check(await t.open(page, `#/node/${HOST}`, /Rack/, 20000), 'the node page has a rack row', (await t.text(page)).slice(0, 300))
  const value = () => page.locator('[data-rack]').innerText()
  const has = (re) => t.until(async () => re.test(await t.text(page)))
  if (PHASE === 'owned') {
    t.check(await has(/set by stormipmi/), 'stormipmi\'s', (await t.text(page)).slice(0, 400))
    t.check(await t.until(async () => (await value()) === 'rack-ipmi'), 'its value', await value())
    t.check(/location says other/.test(await t.text(page)) === false, 'no disagreement to say')
    t.check((await page.getByRole('button', { name: /^(Set|Change|Clear)$/ }).count()) === 0, 'nothing to edit here')
    await page.getByRole('link', { name: 'Change it on Machines' }).click()
    t.check(await has(/Machines/), 'it leads to Machines')
    return
  }
  if (PHASE === 'hand-reader') {
    t.check(await has(/needs the admin role/), 'a reader is told why')
    t.check((await page.getByRole('button', { name: /^(Set|Change|Clear)$/ }).count()) === 0, 'and has nothing to press')
    return
  }
  await page.getByRole('button', { name: /^(Set|Change)$/ }).click()
  await page.locator('input[aria-label=Rack]').fill(PHASE === 'hand-bob' ? 'bobs-rack' : 'bad rack')
  await page.getByRole('button', { name: 'Save' }).click()
  if (PHASE === 'hand-bob') {
    t.check(await page.locator('[role=alert]').waitFor({ timeout: 8000 }).then(() => true, () => false), 'an admin here the apiserver refuses: said')
    t.check(/forbidden|not allowed/i.test(await page.locator('[role=alert]').innerText()), 'in its words', await page.locator('[role=alert]').innerText())
    return
  }
  t.check(await has(/up to 63 letters/), 'not a label value: said', (await t.text(page)).slice(0, 300))
  await page.locator('input[aria-label=Rack]').fill('rack-7')
  await page.getByRole('button', { name: 'Save' }).click()
  t.check(await t.until(async () => (await value()) === 'rack-7'), 'set, and read back', await value())
  await t.shot(page, 'node-rack')
  await page.getByRole('button', { name: 'Clear' }).click()
  t.check(await t.until(async () => (await value()) === 'none'), 'cleared', await value())
  await page.getByRole('button', { name: 'Set' }).click()
  await page.locator('input[aria-label=Rack]').fill('rack-7')
  await page.getByRole('button', { name: 'Save' }).click()
  t.check(await t.until(async () => (await value()) === 'rack-7'), 'set again for the Drives page')
})
