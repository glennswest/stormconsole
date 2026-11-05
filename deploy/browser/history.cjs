// The History page in a browser (#121), over deploy/verify-history.sh's
// system-data, written by stormblock's /init block, stormpump's Writer and
// stormdrive: boots as one row each, a boot's timeline and kernel log, a crash,
// installs, drives and hardware; a diskless node says it has none.
const { walk } = require('./lib.cjs')
const { BOOT1, BOOT2, BOOT3, ALONE, INSTALL, DRIVE, BARE } = process.env

walk('history', async (t) => {
  const page = await t.page()
  await page.goto(`${t.BASE}/#/`)
  t.check(await t.until(async () => (await page.locator('nav a[href="#/history"]').count()) === 1 || (await page.getByRole('button', { name: /Compute/ }).count()) > 0), 'the navigator has Compute')
  const hrefs = await page.evaluate(async () => (await (await fetch('/api/v1/console/nav')).json()))
  t.check(JSON.stringify(hrefs).includes('#/history'), 'the nav offers History')

  // Boots.
  t.check(await t.open(page, '#/history', /this boot/), 'the page opens on the boots', (await t.text(page)).slice(0, 300))
  let text = await t.text(page)
  t.check(/Kept across every install/.test(text), 'it says what this is')
  t.check((await page.locator('table').last().locator('tbody tr').count()) === 4, 'four boots: paired, not six rows', text.slice(0, 500))
  // The boots' table, not the crashes' above it.
  const row = (id) => page.locator('table').last().locator('tbody tr', { has: page.locator(`a[href*="${id}"]`) }).first()
  t.check(/this boot/.test(await row(BOOT3).innerText()), 'this boot is marked', await row(BOOT3).innerText())
  t.check(/running/.test(await row(BOOT3).innerText()), 'and running')
  t.check(/crashed/.test(await row(BOOT1).innerText()), 'the panicked boot says crashed', await row(BOOT1).innerText())
  t.check(/installed/.test(await row(BOOT2).innerText()) && /shut down/.test(await row(BOOT2).innerText()), 'the install is marked, and it shut down', await row(BOOT2).innerText())
  t.check(/no timeline/.test(await row(ALONE).innerText()), "stormblock's record alone says no timeline", await row(ALONE).innerText())
  t.check(/Kernel crashes/i.test(text) && /Kernel panic - not syncing/.test(text), 'the crash is listed')
  await t.shot(page, 'history-boots')

  // A boot.
  await row(BOOT1).locator(`a[href*="${BOOT1}"]`).first().click()
  t.check(await t.until(async () => /command line/i.test(await t.text(page))), 'a boot opens', page.url())
  text = await t.text(page)
  t.check(/Kernel panic - not syncing/.test(text), 'with its panic line')
  t.check(/stormdrive/.test(text) && /all-running/.test(text), 'its timeline', text.slice(0, 600))
  t.check(/slowest/i.test(text) && /modules/.test(text), 'the slowest phases')
  t.check(/names no boot/.test(text), "says stormblock's record was joined by time")
  t.check(/taken/.test(text), 'the disk verdict')
  t.check(/stormblock\.volume=stormpump/.test(text), 'the command line')
  await page.getByRole('button', { name: /Show the last/ }).click()
  t.check(await t.until(async () => /Linux version/.test(await t.text(page))), 'the kernel log shows', (await t.text(page)).slice(-400))
  t.check(/kernel record\(s\) lost/.test(await t.text(page)), 'with its gap')
  await page.getByLabel('Level').selectOption('3')
  t.check(await t.until(async () => !/Linux version/.test(await t.text(page))), 'errors only leaves out the info lines')
  t.check(/I\/O timeout/.test(await t.text(page)), 'and keeps the error')
  await page.getByLabel('Level').selectOption('7')
  await page.getByLabel('Search this log').fill('LINUX')
  t.check(await t.until(async () => /Linux version/.test(await t.text(page)) && !/I\/O timeout/.test(await t.text(page))), 'search narrows it')
  const dl = await page.locator('a', { hasText: 'Download' }).getAttribute('href')
  t.check(dl.includes(`/boots/${BOOT1}/kernel?download=true`), 'Download takes the whole log', dl)
  await page.getByRole('button', { name: 'dmesg.txt' }).click()
  t.check(await t.until(async () => /Oops: 0002/.test(await t.text(page))), "the crash's dmesg opens")
  await t.shot(page, 'history-boot')

  // Installs, and on to the boot.
  t.check(await t.open(page, '#/history?tab=installs', /12\.61/), 'installs list the install', (await t.text(page)).slice(0, 300))
  text = await t.text(page)
  t.check(/12\.60/.test(text) && /\/dev\/sda/.test(text) && /taken/.test(text), 'what it put down, over what, on which drive', text.slice(0, 500))
  await page.locator(`a[href*="tab=boots&id=${INSTALL}"]`).first().click()
  t.check(await t.until(async () => /command line/i.test(await t.text(page)) && new RegExp(BOOT2).test(await t.text(page))), 'an install reaches its whole boot', (await t.text(page)).slice(0, 300))

  // Drives.
  t.check(await t.open(page, '#/history?tab=drives', /ZC1234/), 'drives list', (await t.text(page)).slice(0, 300))
  await t.shot(page, 'history-drives')
  await page.locator(`a[href*="${DRIVE}"]`).first().click()
  t.check(await t.until(async () => /Counters over these records/i.test(await t.text(page))), 'a drive opens')
  text = await t.text(page)
  t.check(/media_errors grew 32 → 33/.test(text), 'its finding, in the words stormdrive would use', text.slice(0, 700))
  t.check(/went backwards/.test(text), 'a counter gone backwards')
  t.check(/format done/.test(text), 'what happened to it')
  await t.shot(page, 'history-drive')

  // Hardware.
  t.check(await t.open(page, '#/history?tab=assets', /first record|nothing|removed/), 'hardware lists', (await t.text(page)).slice(0, 300))
  await t.shot(page, 'history-assets')
  await page.locator('table tbody tr a[href*="tab=assets"]').first().click()
  t.check(await t.until(async () => /This boot's hardware/i.test(await t.text(page))), 'a record opens')
  t.check((await page.locator('section.card').count()) > 1, 'its parts, grouped', (await t.text(page)).slice(0, 400))
  await t.shot(page, 'history-asset')

  // Something that is not there: said on the page.
  await page.goto(`${t.BASE}/#/history?tab=boots&id=nope`)
  t.check(await t.until(async () => /no boot nope/.test(await t.text(page))), 'a boot that is not there is said')

  // A diskless node.
  const bare = await t.page()
  await bare.goto(`${BARE}/#/history`)
  t.check(await t.until(async () => /No system-data on this node/.test(await t.text(bare))), 'a diskless node says so', (await t.text(bare)).slice(0, 200))
  t.check(/diskless/.test(await t.text(bare)), 'and why')
  const nav = await bare.evaluate(async () => (await (await fetch('/api/v1/console/nav')).json()))
  t.check(!JSON.stringify(nav).includes('#/history'), 'with no History on its navigator')
  await t.shot(bare, 'history-bare')
})
