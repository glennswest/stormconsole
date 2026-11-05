// Offered drives in a browser (#153), over deploy/verify-drive-offer.sh's real
// engine and apiserver.
const { walk } = require('./lib.cjs')
const { BLANK, FOREIGN, WORD } = process.env

walk('offered drives', async (t) => {
  const band = (p) => p.locator('section[aria-label="Offered drives"]')
  const ask = (p) => p.locator('section[aria-label="Format a drive"]')
  const row = (p, name) => band(p).locator('tbody tr', { hasText: name }).first()

  // bob may read, and is offered nothing to press.
  const bob = await t.page('bob')
  t.check(await t.open(bob, '#/drives', /Offered drives/), 'bob sees the offered drives', (await t.text(bob)).slice(0, 300))
  t.check(/GPT partition table/.test(await row(bob, FOREIGN).innerText()), 'and what the foreign one holds', await row(bob, FOREIGN).innerText())
  t.check((await band(bob).locator('button').count()) === 0, 'with no Format for him')
  t.check(/storage-admin/.test(await band(bob).innerText()), 'and is told whose it is')

  const page = await t.page('alice')
  t.check(await t.open(page, '#/drives', /Offered drives/), 'alice sees them', (await t.text(page)).slice(0, 300))
  const text = await band(page).innerText()
  t.check(/blank, nothing found/.test(await row(page, BLANK).innerText()), 'the blank drive says so', text)
  t.check(/holds .*GPT partition table/.test(await row(page, FOREIGN).innerText()), 'the foreign one says what it holds', text)
  t.check(/a power cut/i.test(text), 'and the band says why nothing formats on its own')
  t.check((await band(page).locator('tbody tr').count()) === 2, 'only the two offered, not the adopted one', text)
  await t.shot(page, 'offer-band')

  // The filter and a picked drive: the join with stormdrive's record.
  await page.getByLabel('Filter').selectOption('offered')
  t.check(await t.until(async () => /1 drives?|Offered/.test(await t.text(page))), 'the Offered filter is there')
  const cell = page.locator('.cell, button.bay, [data-drive]').first()
  if (await cell.count()) await cell.click().catch(() => {})
  if (await page.locator('.offerline').count()) {
    t.check(/Offered: (holds|blank)|ours:/.test(await page.locator('.offerline').innerText()), 'a picked drive says its offer', await page.locator('.offerline').innerText())
  }

  // The foreign drive: cancelled, nothing happens.
  await row(page, FOREIGN).getByRole('button', { name: /Format/ }).click()
  t.check(await t.until(async () => /erases what it holds/.test(await ask(page).innerText())), "the engine's risk is shown", await ask(page).innerText())
  const said = await ask(page).innerText()
  t.check(/GPT partition table/.test(said) && /power cut/i.test(said), 'naming what goes and the power cut', said)
  t.check(!/send|"confirm"/.test(said), 'without the API instruction', said)
  const go = ask(page).getByRole('button', { name: 'Format this drive' })
  t.check(await go.isDisabled(), 'Format is off until the word is typed')
  await ask(page).getByLabel("The drive's name, typed").fill('not-it')
  t.check(await go.isDisabled(), 'and stays off for the wrong word')
  await t.shot(page, 'offer-risk')
  await ask(page).getByRole('button', { name: 'Cancel' }).click()
  t.check(await t.until(async () => (await ask(page).count()) === 0), 'Cancel closes it')
  t.check(/Format/.test(await row(page, FOREIGN).innerText()), 'and it is still offered')

  // The blank drive: formatted.
  await row(page, BLANK).getByRole('button', { name: /Format/ }).click()
  t.check(await t.until(async () => /erases what it holds/.test(await ask(page).innerText())), 'the blank drive asks too')
  await ask(page).getByLabel("The drive's name, typed").fill(WORD)
  t.check(await go.isEnabled(), 'the right word turns it on')
  await go.click()
  t.check(await t.until(async () => /is a data slab now/.test(await t.text(page)), 60000), 'formatted', (await t.text(page)).slice(0, 400))
  t.check(await t.until(async () => (await row(page, BLANK).count()) === 0, 15000), 'and it leaves the offered list, from the feed')

  // Then a rebalance, a dry run first.
  const reb = page.locator('section[aria-label="Rebalance"]')
  t.check(await t.until(async () => /Rebalance: a dry run/i.test(await reb.innerText()), 30000), 'a dry run follows', await reb.innerText().catch(() => ''))
  const plan = await reb.innerText()
  await t.shot(page, 'offer-plan')
  t.check(/extent\(s\) would move/.test(plan), 'it says what would move, so a real run can follow', plan)
  await reb.getByRole('button', { name: 'Run this rebalance' }).click()
  t.check(await t.until(async () => /Rebalance (running|done): \d+ of \d+ moved/.test(await reb.innerText()), 30000), 'the run reports its progress', await reb.innerText())
  t.check(await t.until(async () => /Rebalance done/.test(await reb.innerText()), 120000), 'and finishes', await reb.innerText())
  await t.shot(page, 'offer-done')

  // From the engine's own list, where any row button goes through api.js: the
  // gate's word, then the engine's risk, then the format. The dialog is accepted.
  const row2 = await t.page('alice')
  row2.answer = WORD
  const seen = []
  row2.on('dialog', (d) => seen.push(`${d.type()}: ${d.message()}`))
  t.check(await t.open(row2, '#/grid?id=sb:engine&rel=drives', /foreign\.img/), 'the engine list shows the drive', (await t.text(row2)).slice(0, 300))
  const r2 = row2.locator('tr', { hasText: 'foreign.img' }).first()
  t.check(/offered: holds/.test(await r2.innerText()), 'and the offer', await r2.innerText())
  await r2.getByRole('button', { name: /actions|more|⋮/i }).first().click().catch(async () => r2.locator('button').last().click())
  await row2.getByRole('button', { name: /Format as a slab/ }).first().click()
  t.check(await t.until(async () => seen.some((s) => s.startsWith('confirm') && /power cut/i.test(s)), 15000), "the row's button shows the engine's risk too", seen.join(' | ').slice(0, 400))
  t.check(seen.some((s) => s.startsWith('prompt')), 'after asking for the word')
  t.check(await t.until(async () => !(await row2.evaluate(() => fetch('/api/v1/components').then((r) => r.json()))).some((c) => c.label.endsWith('foreign.img') && c.actions.some((a) => a.id === 'format')), 15000), 'and it formats, once both are given')
})
