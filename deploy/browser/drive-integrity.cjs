// RAID set integrity (#368): the alarm bar on any page names md0's bit rot and opens the
// Integrity page on it; the page shows the set's counters, lifetime and the day, its scrub and
// last pass, BitRot with the members it names, and each member's drive; a member opens its
// drive, whose Integrity panel shows the member's counters. A drive in no set says so.
// The node engine's word on a drive (#394): NET003's level, its PI/EEDP/sense rows with their
// rates, a red alarm; the map's Drive errors mode and the bay's mark in the others.
const { walk } = require('./lib.cjs')

walk('drive integrity', async (t) => {
  const page = await t.page()
  // This rig's console hears no fleet, so another node's charts are a 503 the page says in words (#369, #395):
  // the drive's Link trend falls back to the samples kept, a set's trend says why it has none.
  page.expect5xx = (path) => /^\/api\/plugins\/metrics\/charts\?.*\bon=/.test(path)

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
  // md0 is on forge1, and this console hears no fleet: its trend says so, not an empty chart (#385, #395).
  t.check(await t.until(async () => /does not hear the fleet/.test(await page.getByTestId('integrity-trend').innerText()), 15000), "md0's trend says why it cannot be read here", await page.getByTestId('integrity-trend').innerText().catch(() => ''))
  t.check((await page.getByTestId('integrity-trend').locator('section.chart').count()) === 0, 'and draws no empty chart')
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
  // What its node's engine sees (#394): the level, the why, the PI/EEDP/sense rows, the rate now.
  t.check(await t.until(async () => (await page.getByTestId('drive-errors-level').count()) === 1, 15000), 'the heading marks the errors level')
  t.check((await page.getByTestId('drive-errors-level').innerText()) === 'errors failing', 'errors failing', await page.getByTestId('drive-errors-level').innerText())
  const line = await page.getByTestId('drive-errors-line').innerText()
  t.check(/failing · since .* UTC · 2 samples running with a PI check or medium error · 2 defects grown in the last day/.test(line), 'the level, since, streak and defects a day', line)
  t.check(/Why: pi_guard 0\.5\/min; read_uncorrected 0\.0\/min/.test(line), "and the engine's why", line)
  const eng = page.getByTestId('drive-errors-counters')
  t.check((await cell(eng, 'piGuard', 1)) === '4' && (await cell(eng, 'piRefTag', 1)) === '1' && (await cell(eng, 'eedpGuard', 1)) === '1' && (await cell(eng, 'nonMedium', 1)) === '3', 'counted: PI guard 4, ref tag 1, EEDP guard 1, non-medium 3')
  t.check((await cell(eng, 'piGuard', 2)) === '2' && (await cell(eng, 'piGuard', 3)) === '0.5/min', 'PI guard: 2 since the console began, 0.5/min now')
  t.check(/error/.test(await eng.locator('tr[data-key="piGuard"] td').nth(2).getAttribute('class')), 'a PI check rise in red')
  t.check((await eng.locator('tr[data-key="verifyRetryCorrected"]').count()) === 0, 'no verify page, no row')
  t.check((await cell(own, 'readUncorrected', 3)) === '0.03/min', "the drive's own rows get the engine's rate")
  await t.shot(page, 'drive-panel')
  await panel.getByTestId('drive-integrity-set').click()
  t.check(await t.until(async () => /#\/drives\/integrity\?set=md0/.test(page.url()), 5000), 'the panel links to the set')

  // NET002 (bay 1) is in no set.
  await page.goto(`${t.BASE}/#/drives?drive=${encodeURIComponent('drive:drive:5000cca26a819e62')}`)
  t.check(await t.until(async () => /In no RAID set/.test(await page.getByTestId('drive-integrity').innerText()), 15000), 'a drive in no set says so')
  t.check((await page.getByTestId('drive-own').count()) === 0, 'NET002 gives no counters: no table of them')

  // NET004 gives none of its own, but its node's engine samples it (#394): its rows, ok, no mark.
  await page.goto(`${t.BASE}/#/drives?drive=${encodeURIComponent('drive:drive:5000cca26a819e64')}`)
  t.check(await t.until(async () => (await page.getByTestId('drive-errors-counters').count()) === 1, 15000), "NET004: the engine's rows alone")
  t.check((await page.getByTestId('drive-errors-level').count()) === 0, 'ok: no mark in the heading')
  t.check(/^Its node’s engine: ok/.test(await page.getByTestId('drive-errors-line').innerText()), 'the line says ok')

  // The map (#394): NET003's bay marked along its head, the chip, the shelf's line, the colour mode.
  await page.goto(`${t.BASE}/#/drives`)
  t.check(await t.until(async () => (await page.locator('button.bay.errfail').count()) === 1, 15000), 'one bay marked: drive errors failing')
  t.check(/1 drive errors \(1 failing\)/.test(await page.getByTestId('errors-chip').innerText()), 'the chip counts it', await page.getByTestId('errors-chip').innerText())
  t.check((await page.getByTestId('group-errors-failing').count()) === 1, "the shelf's line says so")
  t.check(/drive errors failing/.test(await page.locator('button.bay.errfail').getAttribute('title')), 'and the bay says so')
  await page.getByLabel('Colour by').selectOption('errors')
  const bg = (label) => page.locator(`button.bay[data-drive^="${label}"]`).first().evaluate((e) => e.style.background)
  t.check(/--error/.test(await bg('sdc')), 'coloured by drive errors: bay 3 red', await bg('sdc'))
  t.check(/--ok/.test(await bg('sdd')), "NET004's engine ok: green", await bg('sdd'))
  t.check(!/--ok|--error|--warn/.test(await bg('sdb')), 'NET002, none reported: not green', await bg('sdb'))
  await t.shot(page, 'errors-mode')
  await page.getByTestId('errors-chip').click()
  t.check(await t.until(async () => (await page.locator('button.bay').count()) === 1, 5000), 'the chip filters to that drive')

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
