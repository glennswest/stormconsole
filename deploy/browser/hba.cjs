// The SAS cards page and Flash in a browser (#376), over deploy/verify-hba.sh: forge1's HBA330 as a
// HostBusAdapter, sdb at 12 Gbit and sdc held at 6 Gbit (link degraded) on phys 0 and 1, an earlier BIOS flash
// and a dry run the node refused. STATUS is a command that writes a CardOperation's status as stormdrive's
// controller does: `STATUS <name> <json>`.
const { execFileSync } = require('node:child_process')
const { walk } = require('./lib.cjs')

const CARD = process.env.CARD
const status = (name, st) => execFileSync(process.env.STATUS, [name, JSON.stringify(st)])

walk('hba', async (t) => {
  const page = await t.page('alice')

  // The page: the card, its identity, its phys with each drive's link, its regions, its operations.
  t.check(await t.open(page, '#/drives/hbas', /HBA330 Mini/), 'alice opens SAS cards')
  const card = page.locator(`[data-testid=hba][data-name="${CARD}"]`)
  t.check(await t.until(async () => (await card.count()) === 1), 'forge1’s card')
  const id = await card.locator('[data-testid=hba-identity]').innerText()
  t.check(/16\.17\.00\.03/.test(id) && /08\.37\.00\.00/.test(id) && /0e\.01\.00\.07/.test(id) && /500605b00c1d2e00/.test(id), 'firmware, BIOS, NVDATA and SAS address', id)
  t.check(/SAS3008/.test(id) && /MPI 2\.5/.test(id), 'the chip and IOC Facts', id)
  const phy = (n) => card.locator(`[data-testid=hba-phy][data-phy="${n}"]`)
  t.check((await phy(0).innerText()).includes('12 Gbit') && /sdb/.test(await phy(0).innerText()), 'phy 0: 12 Gbit, sdb', await phy(0).innerText())
  t.check(await t.until(async () => /degraded/.test(await phy(1).innerText())), 'phy 1: sdc, its link degraded', await phy(1).innerText())
  t.check(/6 Gbit/.test(await phy(1).innerText()) && /1\.5–12 Gbit/.test(await phy(1).innerText()), 'at 6 of the 1.5–12 Gbit the card allows')
  t.check(/no link/.test(await phy(2).innerText()) && /disabled/.test(await phy(3).innerText()), 'phy 2 no link, phy 3 disabled')
  const imgs = await card.locator('[data-testid=hba-image]').count()
  t.check(imgs === 5, 'five regions read back', String(imgs))
  t.check(/a1a1a1/.test(await card.locator('[data-testid=hba-images]').innerText()), 'each with its sha256')
  const ops = card.locator('[data-testid=hba-op]')
  t.check(await t.until(async () => (await ops.count()) === 2), 'two operations: the dry run and the earlier BIOS flash')
  const first = ops.first()
  t.check((await first.getAttribute('data-phase')) === 'Refused', 'the newest: the dry run, refused')
  t.check(/cordon the Node forge1/.test(await first.locator('[data-testid=hba-op-message]').innerText()), 'with the node’s words', await first.innerText())
  t.check(/08\.35\.00\.00 → 08\.37\.00\.00/.test(await ops.nth(1).innerText()), 'the BIOS flash: before → after')
  await t.shot(page, 'hba-card')

  // Flash: the dialog, the store's image with its sha256, the source asked for, the card's name typed.
  const flash = card.locator('[data-testid=hba-flash]')
  t.check(await t.until(async () => !(await flash.isDisabled())), 'Flash… offered to alice')
  await flash.click()
  const dlg = page.locator('[data-testid=flash-dialog]')
  t.check(await t.until(async () => dlg.isVisible()), 'the dialog')
  const opts = await dlg.locator('[data-testid=flash-image] option').allInnerTexts()
  t.check(opts.some((o) => /hba330-16\.17\.01\.00\.bin/.test(o)) && opts.some((o) => /mptsas3-uefi/.test(o)), 'forge1’s store images offered', opts.join(' | '))
  await dlg.locator('[data-testid=flash-image]').selectOption('hba330-16.17.01.00.bin')
  t.check(/sha256 [0-9a-f]{64}/.test(await dlg.locator('[data-testid=flash-sha]').innerText()), 'its sha256 shown')
  t.check(/recorded from/.test(await dlg.locator('[data-testid=flash-why]').innerText()), 'the source is asked for', await dlg.locator('[data-testid=flash-why]').innerText().catch(() => ''))
  await dlg.locator('[data-testid=flash-source]').fill('https://dl.example/hba330-16.17.01.00.bin')
  await dlg.locator('[data-testid=flash-dry]').uncheck()
  t.check(await t.until(async () => (await dlg.locator('[data-testid=flash-word]').innerText().catch(() => '')) === CARD), 'the word is the card’s name')
  t.check(/Flash/.test(await dlg.locator('[data-testid=flash-go]').innerText()), 'the button says Flash, not Dry run')
  await dlg.locator('[data-testid=flash-typed]').fill('sdb')
  t.check(await dlg.locator('[data-testid=flash-go]').isDisabled(), 'the wrong word does nothing')
  await dlg.locator('[data-testid=flash-typed]').fill(CARD)
  await t.shot(page, 'hba-flash-dialog')
  await dlg.locator('[data-testid=flash-go]').click()
  t.check(await t.until(async () => /flash of firmware with hba330-16\.17\.01\.00\.bin asked for/.test(await page.locator('[data-testid=hba-note]').innerText().catch(() => ''))), 'asked for', await page.locator('[data-testid=hba-note]').innerText().catch(() => ''))
  t.check(await t.until(async () => (await ops.count()) === 3), 'the new operation listed')
  const op = await ops.first().getAttribute('data-name')
  t.check(/^forge1-0000-01-00-0-firmware-\d+$/.test(op || ''), 'named by the card, the region and the time', op)
  t.check(await t.until(async () => /one flash at a time/.test(await card.locator('[data-testid=hba-flash-why]').innerText().catch(() => ''))), 'Flash withheld while it waits', await card.locator('[data-testid=hba-flash-why]').innerText().catch(() => ''))

  // The node runs it: progress, then before → after.
  status(op, { phase: 'Running', step: 'download', bytesDone: 18432, bytesTotal: 73728, requester: 'alice', before: '16.17.00.03', imageVersion: '16.17.01.00', started: '2026-11-19T12:00:00Z' })
  t.check(await t.until(async () => /download 25%/.test(await ops.first().innerText())), 'running: download 25%', await ops.first().innerText())
  t.check(/16\.17\.00\.03 → 16\.17\.01\.00/.test(await ops.first().locator('[data-testid=hba-op-versions]').innerText()), 'from → to while it runs')
  await t.shot(page, 'hba-running')
  status(op, { phase: 'Succeeded', step: 'done', message: 'flashed: 16.17.01.00 running after the reset', requester: 'alice', before: '16.17.00.03', after: '16.17.01.00', finished: '2026-11-19T12:01:30Z' })
  t.check(await t.until(async () => (await ops.first().getAttribute('data-phase')) === 'Succeeded'), 'succeeded')
  t.check(/16\.17\.00\.03 → 16\.17\.01\.00/.test(await ops.first().locator('[data-testid=hba-op-versions]').innerText()), 'before → after')
  t.check(/alice/.test(await ops.first().innerText()), 'by alice')
  t.check(await t.until(async () => !(await flash.isDisabled())), 'Flash offered again once it ended')

  // A drive's row leads to its card, and the card's phy back to the drive.
  t.check(await t.open(page, '#/drives', /drives/i), 'Drives')
  const bay = page.locator('button.bay[data-drive^="sdc "]').first()
  await bay.click()
  const line = page.locator('[data-testid=picked-hba]')
  t.check(await t.until(async () => /HBA330 Mini, phy 1 at 6 Gbit \(the card allows 12 Gbit\)/.test(await line.innerText())), 'sdc: on the HBA330, phy 1, held below the card', await line.innerText().catch(() => ''))
  await line.locator('a').click()
  t.check(await t.until(async () => page.url().includes(`#/drives/hbas?card=${CARD}`)), 'to its card')
  t.check(await t.until(async () => (await page.locator('[data-testid=hba]').count()) === 1), 'that card alone')
  await page.locator('[data-testid=hba-phy-drive]').first().click()
  t.check(await t.until(async () => page.url().includes('#/drives?drive=')), 'and its phy back to the drive')

  // 390 px.
  await t.open(page, '#/drives/hbas', /HBA330 Mini/)
  await page.setViewportSize({ width: 390, height: 900 })
  await t.pause(500)
  const wide = await page.evaluate(() => document.documentElement.scrollWidth)
  t.check(wide <= 391, 'the page fits 390 px', String(wide))
  await t.shot(page, 'hba-390')

  // bob, a storage-viewer: reads it all, offered no flash, told why.
  const bob = await t.page('bob')
  t.check(await t.open(bob, '#/drives/hbas', /HBA330 Mini/), 'bob opens SAS cards')
  t.check(await t.until(async () => /not a storage-admin/.test(await bob.locator('[data-testid=hba-flash-why]').innerText().catch(() => ''))), 'bob: Flash withheld, not a storage-admin')
  t.check(await bob.locator('[data-testid=hba-flash]').isDisabled(), 'the button off')

  // carol, no storage role: the apiserver's refusal.
  const carol = await t.page('carol')
  t.check(await t.open(carol, '#/drives/hbas', /could not be read/), 'carol: SAS cards could not be read')
})
