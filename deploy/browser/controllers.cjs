// The Controllers page and the drive map's "Failed by the controller" in a browser (#325), over
// deploy/verify-controllers.sh: forge1's PERC H730 in RAID mode with no perccli, sda the system drive,
// sdb-sdd failed by the controller, slot 7 a disk with no drive record, slot 8 a hot spare. PERCCLI_ON is a
// command the walk runs (through the rig) to put perccli on the node.
const { execSync } = require('node:child_process')
const { walk } = require('./lib.cjs')

walk('controllers', async (t) => {
  const page = await t.page('alice')
  const prompts = []
  page.on('dialog', (d) => d.type() === 'prompt' && prompts.push(d.message()))

  // The Drives page: the banner, and sdb said as failed by the controller, never missing.
  t.check(await t.open(page, '#/drives', /drives/i), 'alice opens Drives')
  const banner = page.locator('[data-testid=controller-banner][data-code=raid_mode]')
  t.check(await t.until(async () => (await banner.count()) > 0), 'the RAID-mode banner on Drives')
  t.check(/^HBA mode is required/.test(await banner.first().innerText()), 'it says HBA mode is required', await banner.first().innerText().catch(() => ''))
  const bay = page.locator('button.bay[data-drive^="sdb "]').first()
  t.check(await t.until(async () => /Failed by the controller/.test((await bay.getAttribute('title')) || '')), "sdb's bay: Failed by the controller", await bay.getAttribute('title').catch(() => ''))
  t.check(!/missing/i.test((await bay.getAttribute('title')) || ''), 'not "missing"')
  await bay.click()
  t.check(await t.until(async () => (await page.locator('[data-testid=picked-controller]').count()) > 0), 'the picked drive: Failed by the controller')
  t.check(/Set good/.test(await page.locator('[data-testid=picked-controller-line]').innerText()), 'with the way back: Set good')
  await t.shot(page, 'drives-failed')

  // Set good from the Actions menu: the Controller group first, asked as a storage-admin's, not "destroys data".
  await page.locator('.picked [data-testid=drive-actions]').first().click()
  const m = page.locator('[role=menu]').first()
  t.check(await t.until(async () => m.isVisible()), 'the Actions menu opens')
  const groups = (await m.locator('.gname').allInnerTexts()).map((n) => n.toLowerCase())
  t.check(groups[0] === 'controller', 'the Controller group comes first', groups.join(' | '))
  page.answer = 'NET002'
  await m.getByRole('menuitem', { name: /^Set good/ }).click()
  t.check(await t.until(async () => prompts.length > 0), 'the serial is asked for')
  t.check(/NET002/.test(prompts[0] || '') && !/destroys data/.test(prompts[0] || ''), 'naming sdb’s serial, and not saying it destroys data', prompts[0])
  page.answer = undefined
  t.check(await t.until(async () => /Set good/.test(await page.locator('.actnote').innerText().catch(() => ''))), 'Set good done from the menu', await page.locator('.actnote').innerText().catch(() => ''))

  // The Controllers page.
  t.check(await t.open(page, '#/drives/controllers', /PERC H730 Mini/), 'the Controllers page')
  const ctl = page.locator('[data-testid=controller]').first()
  t.check(/RAID mode/i.test(await ctl.locator('[data-testid=controller-mode]').innerText()), 'the controller: RAID mode')
  t.check(/25\.5\.9\.0001/.test(await ctl.innerText()) && /0000:03:00\.0/.test(await ctl.innerText()), 'with its firmware and address')
  t.check(/^HBA mode is required/.test(await page.locator('[data-testid=controller-banner]').first().innerText()), 'the banner here too')
  const sw = ctl.locator('[data-testid=switch-hba]')
  t.check(await sw.isDisabled(), 'Switch to HBA is off while perccli is not on the node')
  t.check(/perccli is not available on .*stormcentral#1310/.test(await ctl.locator('[data-testid=switch-hba-why]').innerText()), "with perccli's own reason", await ctl.locator('[data-testid=switch-hba-why]').innerText().catch(() => ''))
  const disk = (dev) => ctl.locator(`[data-testid=controller-disk][data-device="${dev}"]`)
  t.check(await t.until(async () => /unconfigured good/i.test(await disk(1).innerText())), 'sdb now unconfigured good (the menu’s Set good)', await disk(1).innerText())
  t.check(/Failed by the controller/.test(await disk(2).innerText()), 'sdc: Failed by the controller')
  t.check(/JBOD: Linux sees it/.test(await disk(0).innerText()), 'sda: JBOD')
  t.check(await disk(0).locator('[data-testid=set-good]').count() === 0, 'sda offers nothing')
  t.check(/a member or spare/.test(await disk(8).innerText()) || (await disk(8).locator('[data-testid=set-good]').count()) === 0, 'the hot spare offers nothing')
  await t.shot(page, 'controllers-raid')

  // Set JBOD on sdc, through the dialog: the serial, a wrong word refused.
  await disk(2).locator('[data-testid=set-jbod]').click()
  const dlg = page.locator('[data-testid=controller-dialog]')
  t.check(await t.until(async () => dlg.isVisible()), 'Set JBOD: the dialog')
  t.check(await t.until(async () => (await dlg.locator('[data-testid=controller-word]').innerText()) === 'NET003'), 'the word is sdc’s serial')
  await dlg.getByLabel('Confirmation').fill('NET002')
  t.check(await dlg.locator('[data-testid=controller-go]').isDisabled(), 'the wrong word does nothing')
  await dlg.getByLabel('Confirmation').fill('NET003')
  await dlg.locator('[data-testid=controller-go]').click()
  t.check(await t.until(async () => /the controller reads jbod/.test(await page.locator('[data-testid=controller-note]').innerText().catch(() => ''))), 'sdc: the controller reads jbod')
  t.check(await t.until(async () => /JBOD: Linux sees it/.test(await disk(2).innerText())), 'sdc: Linux sees it')

  // Slot 7, a disk the inventory has no record of: the controller's own route, confirmed by the controller's card.
  t.check(/no drive record/.test(await disk(7).innerText()) && /Unconfigured bad/.test(await disk(7).innerText()), 'slot 7: no drive record, unconfigured bad')
  await disk(7).locator('[data-testid=set-good]').click()
  t.check(await t.until(async () => (await dlg.locator('[data-testid=controller-word]').innerText().catch(() => '')) === '5CF1234'), 'confirmed by the controller’s serial')
  await dlg.getByLabel('Confirmation').fill('5CF1234')
  await dlg.locator('[data-testid=controller-go]').click()
  t.check(await t.until(async () => /unconfigured good/i.test(await disk(7).innerText())), 'slot 7: good')

  // perccli arrives: Switch to HBA, its reboot said, then the reboot banner and HBA after reboot.
  execSync(process.env.PERCCLI_ON)
  t.check(await t.until(async () => !(await sw.isDisabled()), 20000), 'Switch to HBA offered once perccli is there')
  await sw.click()
  t.check(await t.until(async () => dlg.isVisible()), 'Switch to HBA: the dialog')
  const lines = await dlg.locator('[data-testid=controller-steps]').innerText()
  t.check(/when .* is rebooted/.test(lines) && /No data is touched/.test(lines), 'it says a reboot applies it and no data is touched', lines)
  t.check(await t.until(async () => (await dlg.locator('[data-testid=controller-word]').innerText().catch(() => '')) === '5CF1234'), 'the word: the controller’s serial')
  await t.shot(page, 'switch-hba-dialog')
  await dlg.getByLabel('Confirmation').fill('5CF1234')
  await dlg.locator('[data-testid=controller-go]').click()
  t.check(await t.until(async () => /HBA asked for; reboot/.test(await page.locator('[data-testid=controller-note]').innerText().catch(() => ''))), 'HBA asked for; reboot to apply it')
  t.check(await t.until(async () => (await page.locator('[data-testid=controller-banner][data-code=reboot_pending]').count()) > 0), 'the banner: a reboot applies it')
  t.check(await t.until(async () => (await page.locator('[data-testid=controller-banner][data-code=raid_mode]').count()) === 0), 'and no longer "HBA mode is required"')
  t.check(/HBA after reboot/i.test(await ctl.locator('[data-testid=controller-pending]').innerText().catch(() => '')), 'HBA after reboot on the card')
  t.check(await sw.isDisabled(), 'Switch to HBA not offered twice')
  t.check(/predates controller management/.test(await page.locator('[data-testid=controllers-quiet]').innerText().catch(() => '')), 'storm-b: its stormdrive predates controllers')
  await t.shot(page, 'controllers-pending')

  // 390 px: the page fits.
  await page.setViewportSize({ width: 390, height: 900 })
  await t.pause(500)
  const wide = await page.evaluate(() => document.documentElement.scrollWidth)
  t.check(wide <= 391, 'the page fits 390 px', String(wide))
  await t.shot(page, 'controllers-390')

  // bob, a storage-viewer: sees it all, offered nothing.
  const bob = await t.page('bob')
  t.check(await t.open(bob, '#/drives/controllers', /PERC H730 Mini/), 'bob opens Controllers')
  t.check(await bob.locator('[data-testid=set-good]').first().isDisabled().catch(() => true), 'bob: Set good off')
  const bctl = bob.locator('[data-testid=controller]').first()
  t.check(await t.until(async () => /set good|jbod|perccli|reboot|storage|allowed|HBA/i.test(await bctl.innerText())), 'bob sees the controller')
})
