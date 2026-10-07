// Drives, Pools and Shelves in a browser (#58, for #32 and #29), over
// deploy/verify-drives.sh's ten stand-in stormdrives and engine.
const { walk } = require('./lib.cjs')

walk('drives, pools, shelves', async (t) => {
  const page = await t.page()
  const main = () => t.text(page)

  t.check(await t.open(page, '#/drives', /1,600\s*drives/), 'the map: 1,600 drives', (await main()).slice(0, 200))
  const band = await page.locator('.band').innerText()
  t.check(/10\s*nodes/.test(band) && /chassis/.test(band) && /raw/.test(band), 'the band: nodes, chassis, raw capacity', band)
  t.check(/used .* left of/.test(band), 'and the fleet\'s usage', band)
  t.check((await page.locator('button.bay').count()) > 100, 'bays are drawn', await page.locator('button.bay').count())
  await t.shot(page, 'drives-map')

  // A filter chip narrows the map to the drives that need someone.
  await page.locator('button.chip', { hasText: 'failing' }).click()
  t.check(await t.until(async () => (await page.locator('button.bay').count()) === 1), 'failing: one drive left', await page.locator('button.bay').count())
  t.check(/storm-3/.test(await main()), 'and it is on storm-3')
  await page.locator('button.chip', { hasText: 'failing' }).click()

  // Pick a drive: its usage, its slabs, the volumes on it and who uses them.
  await page.locator('input[aria-label="Search drives"]').fill('here-SN000')
  t.check(await t.until(async () => (await page.locator('button.bay').count()) === 1), 'search finds here-SN000')
  await page.locator('button.bay').first().click()
  const picked = page.locator('section.picked')
  t.check(await t.until(async () => /Volumes on this drive/i.test(await picked.innerText())), 'picking it opens the drive', await picked.innerText().catch(() => ''))
  const p = await picked.innerText()
  t.check(/left/.test(p) && /used/.test(p), 'with its usage', p)
  t.check(/Slabs on this drive/i.test(p), 'its slabs', p)
  t.check(/vm-web-root/.test(p) && /web\/web-1/.test(p) && /pvc-db/.test(p) && /shop\/db/.test(p), 'and its volumes with their consumers', p)
  await t.shot(page, 'drives-picked')
  await picked.getByRole('button', { name: 'Close' }).click()
  await page.locator('input[aria-label="Search drives"]').fill('')

  // The list view and grouping by rack.
  await page.getByRole('button', { name: 'List', exact: true }).click()
  t.check(await t.until(async () => (await page.locator('table tr').count()) > 10), 'the list view has rows', await page.locator('table tr').count())
  await page.locator('select[aria-label="Group by"]').selectOption('rack')
  t.check(await t.until(async () => /\bA\b/.test(await main()) && /\bB\b/.test(await main())), 'grouped by rack: A and B')

  t.check(await t.open(page, '#/drives?group=pool', /storm-1/), 'Pools: a pool per node and tier', (await main()).slice(0, 300))
  const pools = await main()
  t.check(/hot/.test(pools) && /warm/.test(pools), 'with their tiers', pools.slice(0, 300))
  await t.shot(page, 'pools')

  t.check(await t.open(page, '#/drives?group=shelf', /Shelves|shelf/i), 'Shelves', (await main()).slice(0, 200))
  await t.shot(page, 'shelves')
})
