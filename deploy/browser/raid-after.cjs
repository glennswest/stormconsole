// The RAID sets page once verify-raid.sh's set has rebuilt and been scrubbed (#67).
const { walk } = require('./lib.cjs')

walk('raid sets, clean', async (t) => {
  const page = await t.page('alice')
  t.check(await t.open(page, '#/raid', /Shelf ds0/), 'the shelf', (await t.text(page)).slice(0, 300))
  const text = await t.text(page)
  // The rig's boot array is on the page too, on no shelf, and clean throughout.
  const shelf = page.locator('section[aria-label="Shelf ds0"]')
  t.check((await shelf.locator('[data-testid=set][data-state=clean]').count()) === 1, 'clean again', text.slice(0, 300))
  t.check(/Last scrub: no mismatches/.test(text), 'with its last scrub', text.slice(0, 500))
  t.check((await shelf.locator('[data-testid=member][data-state=failed]').count()) === 0, 'and no failed bay')
  await shelf.locator('[data-testid=set]').getByRole('button', { name: 'Scrub' }).click()
  t.check(await t.until(async () => (await page.locator('[data-testid=said]').count()) > 0), 'Scrub answers on the page')
  t.check(/Scrub of ds0-a started/.test(await page.locator('[data-testid=said]').innerText()), 'and says it started', await page.locator('[data-testid=said]').innerText())
  await t.shot(page, 'raid-clean')
})
