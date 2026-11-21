// The integrity trend (#385), over deploy/verify-integrity-trend.sh: md0's counters over time on its set's
// page and on its slot 2 drive's Integrity panel, from forge1's ironprom; the list of sets links to each
// set's page; md1's charts say storm-b's ironprom has no stormraid job; the Metrics page names the set.
const { walk } = require('./lib.cjs')

walk('integrity trend', async (t) => {
  const page = await t.page('root')
  const trend = () => page.getByTestId('integrity-trend')
  const drawn = (scope, n) => t.until(async () => (await scope.locator('section.chart svg').count()) >= n, 30000)

  t.check(await t.open(page, '#/drives/integrity?set=md0', /Over time/), 'md0’s page has its trend', (await t.text(page)).slice(0, 300))
  t.check(await drawn(trend(), 1), 'drawn over the week it opens on', (await trend().innerText().catch(() => '')).slice(0, 300))
  const win = trend().getByLabel('Window')
  t.check((await win.inputValue()) === '604800', 'it opens on a week', await win.inputValue())
  const opts = await win.locator('option').allInnerTexts()
  t.check(opts.includes('30 days') && opts.includes('90 days'), 'and offers 30 and 90 days, as long as ironprom keeps them', opts.join(', '))
  t.check(/From forge1.s ironprom/.test(await trend().getByTestId('charts-from').innerText()), 'from forge1’s ironprom, the set’s node')
  await win.selectOption('900')
  t.check(await drawn(trend(), 6), 'over 15 minutes: every chart drawn', (await trend().innerText()).slice(0, 400))
  for (const title of ['Checksum mismatches, by where found', 'Checksum mismatches, per member', 'Blocks rewritten from parity, per member', 'PI, read and write errors, per member', 'Rows past parity', 'Scrub passes done']) {
    t.check((await trend().locator('section.chart', { has: page.getByRole('heading', { name: title }) }).locator('svg').count()) === 1, title)
  }
  await t.shot(page, 'integrity-trend-set')

  t.check(await t.open(page, '#/drives/integrity', /md1/), 'the list of sets')
  t.check(await t.until(async () => (await page.getByTestId('integrity-trend-link').count()) === 2, 10000), 'each set links to its page for its trend')
  t.check((await page.getByTestId('integrity-trend').count()) === 0, 'and the list draws none')
  await page.locator('[data-testid="integrity-set"][data-name="md0"]').getByTestId('integrity-trend-link').click()
  t.check(await t.until(async () => /set=md0/.test(page.url()), 5000), 'the link opens md0')

  t.check(await t.open(page, '#/drives/integrity?set=md1', /Over time/), 'md1’s page')
  t.check(await t.until(async () => /no job stormraid/.test(await trend().innerText()), 30000), 'its charts say storm-b’s ironprom has no stormraid job', (await trend().innerText().catch(() => '')).slice(0, 300))
  t.check(/stormcos#1466/.test(await trend().innerText()), 'and what brings it')
  t.check((await trend().locator('section.chart svg').count()) === 0, 'and draw nothing')
  await t.shot(page, 'integrity-trend-nojob')

  // NET003 is slot 2 of md0.
  t.check(await t.open(page, `#/drives?drive=${encodeURIComponent('drive:drive:5000cca26a819e69')}`, /Slot 2 of RAID set md0/), 'NET003’s drive page: slot 2 of md0')
  const dt = page.getByTestId('drive-integrity-trend')
  t.check(await drawn(dt, 1), 'its Integrity panel draws its trend', (await dt.innerText().catch(() => '')).slice(0, 300))
  t.check(/From forge1.s ironprom/.test(await dt.getByTestId('charts-from').innerText()), 'from forge1’s ironprom')
  await dt.getByLabel('Window').selectOption('900')
  t.check(await drawn(dt, 3), 'mismatches, rewritten and errors, for this member', (await dt.innerText()).slice(0, 400))
  t.check(/naa\.5000cca26a819e69/.test(await dt.innerText()) && !/naa\.5000cca26a819e61/.test(await dt.innerText()), 'its own drive’s lines only')
  await t.shot(page, 'integrity-trend-drive')

  t.check(await t.open(page, '#/metrics?entity=raidset&node=forge1&set=md0', /RAID set md0 on forge1/), 'the Metrics page names the set')
  t.check(await drawn(page, 1), 'and charts it')
  t.check(/From forge1.s ironprom/.test(await page.getByTestId('charts-from').innerText()), 'from forge1’s')
})
