// RAID set integrity (#368): the alarm bar on any page names md0's bit rot and opens the
// Integrity page on it; the page shows the set's counters, lifetime and the day, its scrub and
// last pass, BitRot with the members it names, and each member's drive; a member opens its
// drive, whose Integrity panel shows the member's counters. A drive in no set says so.
const { walk } = require('./lib.cjs')

walk('drive integrity', async (t) => {
  const page = await t.page()

  // Any page: the bar, red (rows past parity).
  await page.goto(`${t.BASE}/#/`)
  const bar = page.getByTestId('alertbar')
  // The stand-ins' failing link is red too, so md0's may be under "and 1 more".
  await t.until(async () => (await bar.count()) > 0 && (await bar.getByRole('button', { name: /more/ }).count()) > 0, 20000)
  const more = bar.getByRole('button', { name: /more/ })
  if (await more.count()) await more.click()
  t.check(await t.until(async () => /Bit rot on RAID set md0 on forge1/.test(await bar.innerText()), 20000), 'the alarm bar names md0', await bar.innerText().catch(() => ''))
  t.check(/Integrity/.test(await bar.innerText()), 'with a way to the Integrity page')
  await t.shot(page, 'alarm-bar')
  await bar.getByRole('link', { name: /Bit rot on RAID set md0/ }).click()
  t.check(await t.until(async () => (await page.getByTestId('integrity-set').count()) === 1, 15000), 'its link opens the Integrity page on md0 alone')

  const set = page.getByTestId('integrity-set')
  t.check((await page.getByTestId('integrity-level').innerText()) === 'rows past parity', 'the level: rows past parity', await page.getByTestId('integrity-level').innerText())
  t.check(/slot 2 \(\/dev\/sdc\): 8 mismatch\(es\)/.test(await page.getByTestId('integrity-bitrot').innerText()), 'BitRot with the member it names')
  const scrub = await page.getByTestId('integrity-scrub').innerText()
  t.check(/not scrubbing · a pass every 7d · 4 passes done · next 2026-11-27 10:00:00 UTC/.test(scrub), 'the scrub', scrub)
  t.check(/1000 stripes · 7 mismatches · 7 repaired/.test(await page.getByTestId('integrity-pass').innerText()), 'the last pass')
  const cell = async (table, key, n) => (await table.locator(`tr[data-key="${key}"] td`).nth(n).innerText()).trim()
  const totals = page.getByTestId('integrity-totals')
  t.check((await cell(totals, 'mismatches', 1)) === '8' && (await cell(totals, 'repaired', 1)) === '10', 'lifetime: 8 mismatches, 10 rewritten')
  t.check((await cell(totals, 'repaired', 2)) === '3' && (await cell(totals, 'unrecoverableRows', 2)) === '2', 'since the console began: 3 rewritten, 2 rows past parity')
  t.check(/since \d\d:\d\d UTC/.test(await totals.locator('th').nth(2).innerText()), 'the day column says since when it was kept')
  t.check(/error/.test(await totals.locator('tr[data-key="unrecoverableRows"] td').nth(2).getAttribute('class')), 'rows past parity in red')
  const members = page.getByTestId('integrity-members')
  const slot2 = await members.locator('tr[data-slot="2"]').innerText()
  t.check(/NET003/.test(slot2) && /\(\+3\)/.test(slot2), "slot 2 is NET003, with the day's 3 rewritten", slot2)
  await t.shot(page, 'integrity-set')

  // Every set: md1 on storm-b, clean, scrubbing.
  await page.goto(`${t.BASE}/#/drives/integrity`)
  t.check(await t.until(async () => (await page.getByTestId('integrity-set').count()) === 2, 10000), 'the page lists both sets')
  const md1 = page.locator('[data-testid="integrity-set"][data-name="md1"]')
  t.check((await md1.getByTestId('integrity-level').innerText()) === 'clean', 'md1 clean')
  t.check(/scrubbing now/.test(await md1.getByTestId('integrity-scrub').innerText()), 'md1 scrubbing now')
  t.check(/BEE001/.test(await md1.getByTestId('integrity-members').innerText()), "md1's member is storm-b's BEE001")
  await t.shot(page, 'integrity-all')

  // A member opens its drive; the drive's panel.
  await page.goto(`${t.BASE}/#/drives/integrity?set=md0`)
  await page.getByTestId('integrity-set').waitFor()
  await page.getByTestId('integrity-members').locator('tr[data-slot="2"]').getByTestId('integrity-member-drive').click()
  t.check(await t.until(async () => (await page.getByTestId('drive-integrity').count()) === 1, 15000), 'the member opens its drive with the Integrity panel')
  const panel = page.getByTestId('drive-integrity')
  t.check(await t.until(async () => /Slot 2 of RAID set md0/.test(await panel.innerText()), 10000), 'slot 2 of md0', await panel.innerText())
  t.check((await page.getByTestId('drive-integrity-bitrot').count()) === 1, "its set's BitRot is marked")
  const dc = page.getByTestId('drive-integrity-counters')
  t.check((await cell(dc, 'mismatches', 1)) === '8' && (await cell(dc, 'repaired', 1)) === '10' && (await cell(dc, 'piErrors', 1)) === '2' && (await cell(dc, 'readErrors', 1)) === '3', 'lifetime: 8, 10 rewritten, 2 PI, 3 read')
  t.check((await cell(dc, 'repaired', 2)) === '3', 'and 3 rewritten since the console began')
  t.check((await dc.locator('tr[data-key="unrecoverableRows"]').count()) === 0, 'no rows-past-parity row for a member')
  t.check(/L1 read path 1 · L3 scrub 7/.test(await page.getByTestId('drive-integrity-where').innerText()), 'where the mismatches were found')
  // The drive's own counters (#384), above the set's view of it.
  const own = page.getByTestId('drive-own-counters')
  t.check(await t.until(async () => (await own.count()) === 1, 15000), "the panel has the drive's own table")
  t.check(/LOG SENSE/.test(await page.getByTestId('drive-own').innerText()), 'from its log pages')
  t.check((await cell(own, 'readUncorrected', 1)) === '3' && (await cell(own, 'grownDefects', 1)) === '7' && (await cell(own, 'readCorrected', 1)) === '150', 'lifetime: 3 uncorrected reads, 7 grown defects, 150 corrected')
  t.check((await cell(own, 'readUncorrected', 2)) === '1' && (await cell(own, 'grownDefects', 2)) === '2', 'and what rose since the console began')
  t.check(/error/.test(await own.locator('tr[data-key="readUncorrected"] td').nth(2).getAttribute('class')), 'an uncorrected rise in red')
  t.check(/warn/.test(await own.locator('tr[data-key="grownDefects"] td').nth(2).getAttribute('class')), 'grown defects in amber')
  t.check(!/warn|error/.test(await own.locator('tr[data-key="readCorrected"] td').nth(2).getAttribute('class')), 'corrected never coloured')
  t.check((await own.locator('tr[data-key="reallocated"]').count()) === 0, 'no ATA row on a SAS drive')
  await t.shot(page, 'drive-panel')
  await panel.getByTestId('drive-integrity-set').click()
  t.check(await t.until(async () => /#\/drives\/integrity\?set=md0/.test(page.url()), 5000), 'the panel links to the set')

  // NET002 (bay 1) is in no set.
  await page.goto(`${t.BASE}/#/drives?drive=${encodeURIComponent('drive:drive:5000cca26a819e62')}`)
  t.check(await t.until(async () => /In no RAID set/.test(await page.getByTestId('drive-integrity').innerText()), 15000), 'a drive in no set says so')
  t.check((await page.getByTestId('drive-own').count()) === 0, 'NET002 gives no counters: no table of them')

  // storm-b's BEE001: ATA SMART, predicting its failure.
  await page.goto(`${t.BASE}/#/drives?drive=${encodeURIComponent('drive:@storm-b:drive:5000c500aaaa0001')}`)
  t.check(await t.until(async () => (await page.getByTestId('drive-own-counters').count()) === 1, 15000), "another node's drive has its own table")
  t.check(/reallocated sector count past its threshold/.test(await page.getByTestId('drive-own-failure').innerText()), 'its predicted failure, in its words')
  t.check((await page.getByTestId('drive-own-predicted').count()) === 1, 'marked in the heading')
  const bee = page.getByTestId('drive-own-counters')
  t.check((await cell(bee, 'reallocated', 1)) === '8' && (await cell(bee, 'crcErrors', 1)) === '3', 'its reallocated sectors and CRC errors')
  t.check((await bee.locator('tr[data-key="readUncorrected"]').count()) === 0, 'no SAS log page row on a SATA drive')
  await t.shot(page, 'drive-own-sata')
})
