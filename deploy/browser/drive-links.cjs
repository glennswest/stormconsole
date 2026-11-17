// Drive link health (#339): the alarm bar on any page names bay 3 and opens the drive; the
// Drives page marks the bay, counts it in the link chip and the shelf's line, and its Link panel
// shows the rate against the maximum, both sides' counters with rates, stormdrive's
// recommendation and the trend from ironprom. storm-b's degraded drive has its trend from the
// samples the console kept.
const { walk } = require('./lib.cjs')

walk('drive links', async (t) => {
  const page = await t.page()

  // Any page: the bar.
  await page.goto(`${t.BASE}/#/`)
  const bar = page.getByTestId('alertbar')
  t.check(await t.until(async () => /Drive link failing: bay 3/.test(await bar.innerText()), 20000), 'the alarm bar names bay 3 on the overview', await bar.innerText().catch(() => ''))
  await t.shot(page, 'alarm-bar')
  await bar.getByRole('link', { name: /Drive link failing: bay 3/ }).click()
  t.check(await t.until(async () => (await page.getByTestId('drive-link').count()) > 0, 15000), 'its link opens the drive on the Drives page')
  const panel = page.getByTestId('drive-link')
  t.check(await t.until(async () => /12 Gbit of 12 Gbit \(hardware\) maximum/.test(await panel.innerText()), 10000), 'the rate against the maximum', await panel.innerText())
  t.check((await page.getByTestId('link-level').innerText()) === 'failing', 'level failing')
  const text = await panel.innerText()
  t.check(/reseat; if it continues, swap bays or replace/.test(text), "stormdrive's recommendation", text)
  t.check(/500\/min/.test(text) && /saturated/.test(text) && /73,506,628/.test(text), 'the drive side: 500/min, the saturated totals, the loss-of-sync total', text)
  t.check(/host/.test(text) && /phy-1:3/.test(text), 'the host side, its phy', text)
  t.check(
    await t.until(async () => /this node.s ironprom/.test(await page.getByTestId('link-trend-source').innerText()), 15000),
    'the trend is the last 24 h from ironprom',
  )
  t.check((await panel.locator('svg').count()) > 0, 'drawn')
  await t.shot(page, 'link-panel')

  // The bay marked, the chip, the shelf's line.
  t.check((await page.locator('button.bay.linkfail').count()) === 1, 'one bay marked failing on the map')
  t.check((await page.locator('button.bay.linkwarn').count()) === 1, "and storm-b's marked degraded")
  t.check(/2 link \(1 failing\)/.test(await page.getByTestId('link-chip').innerText()), 'the chip counts them')
  t.check((await page.getByTestId('group-link-failing').count()) === 1, "the shelf's line says 1 link failing")

  // The SAS link colour mode: ok green, failing red.
  await page.getByLabel('Colour by').selectOption('link')
  const bg = (label) => page.locator(`button.bay[data-drive^="${label}"]`).first().evaluate((e) => e.style.background)
  t.check(/--error/.test(await bg('sdc')), 'coloured by link: bay 3 red', await bg('sdc'))
  t.check(/--ok/.test(await bg('sda')), 'an ok link green', await bg('sda'))
  await t.shot(page, 'link-mode')

  // The list: the level with the rate.
  await page.getByRole('button', { name: 'List' }).click()
  t.check(await t.until(async () => /failing · 500\/min sync/.test(await t.text(page)), 8000), 'the list says failing · 500/min')

  // storm-b's drive, through its alarm: the kept samples.
  await page.goto(`${t.BASE}/#/drives?drive=${encodeURIComponent('drive:@storm-b:drive:5000c500aaaa0001')}`)
  await page.getByTestId('drive-link').waitFor()
  t.check((await page.getByTestId('link-level').innerText()) === 'degraded', "storm-b's: degraded")
  t.check(await t.until(async () => /Reading the link|the cable or the expander port/.test(await page.getByTestId('drive-link').innerText()), 3000), 'before the links are read it claims nothing of the drive')
  await t.until(async () => /the cable or the expander port/.test(await page.getByTestId('drive-link').innerText()), 15000)
  const other = await page.getByTestId('drive-link').innerText()
  t.check(/the cable or the expander port/.test(other) && /expander/.test(other), 'both sides moving: the cable or the expander port', other)
  t.check(!/predates/.test(other), 'and never says its stormdrive is old', other)
  t.check(
    await t.until(async () => /samples this console kept/.test(await page.getByTestId('link-trend-source').innerText()), 10000),
    "another node's trend from the samples the console kept",
  )
  await t.shot(page, 'other-node')

  // ?filter=link, the bar's "Drive links".
  await page.goto(`${t.BASE}/#/drives?filter=link`)
  // The walk left the page in list view, which it remembers.
  await page.getByRole('button', { name: 'Map' }).click()
  t.check(await t.until(async () => (await page.locator('button.bay').count()) === 2, 8000), 'the filter shows the two links', `${await page.locator('button.bay').count()}`)
})
