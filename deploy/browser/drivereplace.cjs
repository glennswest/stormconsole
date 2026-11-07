// Replace… and Update firmware… in a browser (#317), over deploy/verify-drive-replace.sh. This node is laid
// out as forge1 is (sda the system drive, sdb-sdd free), with a fleet drive (sde), an LVM disk (sdf), RAID
// set ds0-a's members (sdk, sdm) and a blank drive at the set's format (sdl). Runs take RUN seconds.
const { walk } = require('./lib.cjs')

const RUN = Number(process.env.RUN || 25) * 1000

walk('drive replace and firmware', async (t) => {
  const page = await t.page('alice')
  t.check(await t.open(page, '#/drives', /drives/i), 'alice opens Drives')
  const header = page.locator('.picked header').first()
  const bay = (name) => page.locator(`button.bay[data-drive^="${name} "]`).first()

  async function pick(name) {
    const b = bay(name)
    t.check(await t.until(async () => (await b.count()) > 0), `${name}'s bay on the map`)
    if (!new RegExp(`^${name} `).test(await header.innerText().catch(() => ''))) await b.click()
    t.check(await t.until(async () => new RegExp(`^${name} `).test(await header.innerText())), `${name} picked`)
  }
  async function menu(width) {
    await page.locator('.picked [data-testid=drive-actions]').first().click()
    const m = page.locator('[role=menu]').first()
    t.check(await t.until(async () => m.isVisible()), 'the Actions menu opens')
    if (width) {
      const mb = await m.boundingBox()
      t.check(mb && mb.x >= 0 && mb.x + mb.width <= width + 1, `inside the window at ${width} px`, JSON.stringify(mb))
    }
    return m
  }
  const item = (m, re) => m.getByRole('menuitem', { name: re })
  async function openReplace(name) {
    await pick(name)
    const m = await menu()
    await item(m, /^Replace…/).click()
    const d = page.locator('[data-testid=replace-dialog]')
    t.check(await t.until(async () => d.isVisible()), `Replace ${name}: the dialog opens`)
    return d
  }
  const cand = (d, name) => d.locator(`[data-testid=replace-candidate][data-drive="${name}"]`)

  // Both are in the menu, before the destructive group, inside the window at 1280 and 390 px.
  for (const width of [1280, 390]) {
    await page.setViewportSize({ width, height: 900 })
    await pick('sdb')
    const m = await menu(width)
    const names = (await m.locator('.gname').allInnerTexts()).map((n) => n.toLowerCase())
    const items = await m.locator('[role=menuitem]').allInnerTexts()
    t.check(names.indexOf('maintain') >= 0 && names.indexOf('maintain') < names.length - 1, `Maintain sits before the destructive group at ${width} px`, names.join(' | '))
    t.check(items.some((i) => /^Replace…/.test(i)) && items.some((i) => /^Update firmware…/.test(i)), `Replace… and Update firmware… at ${width} px`, items.join(' | '))
    await page.keyboard.press('Escape')
    if (width === 390) {
      const d = await openReplace('sdb')
      const box = await d.boundingBox()
      t.check(box && box.x >= 0 && box.x + box.width <= 391, 'the Replace dialog fits 390 px', JSON.stringify(box))
      await t.shot(page, 'replace-390')
      await d.getByRole('button', { name: 'Cancel' }).click()
    }
  }
  await page.setViewportSize({ width: 1440, height: 1000 })

  // sda, the system drive: Replace… is off, and says why.
  await pick('sda')
  let m = await menu()
  const sdaItem = item(m, /^Replace…/)
  t.check(await sdaItem.isDisabled(), "sda's Replace… is off")
  t.check(/system drive.*stormblock#563/.test(await sdaItem.innerText()), 'saying why, and what would let it', await sdaItem.innerText())
  await page.keyboard.press('Escape')

  // sdk, a RAID member: the engine's live replace onto sdl, after sdc is turned down for its format.
  let d = await openReplace('sdk')
  t.check(/RAID member/i.test(await d.locator('[data-testid=replace-kind]').innerText()), 'sdk reads RAID member')
  t.check(await t.until(async () => (await cand(d, 'sdl').count()) > 0), 'sdl is offered')
  t.check(/format sdc to 4096 \+ PI type 1 first/.test(await cand(d, 'sdc').innerText()), "sdc is turned down: not the set's format", await cand(d, 'sdc').innerText())
  t.check((await cand(d, 'sdc').locator('[data-testid=replace-format-first]').count()) === 1, 'with a way to format it first')
  t.check(/not a blank drive this node's engine has open/.test(await cand(d, 'sdb').innerText()), "sdb is turned down: the engine does not have it open")
  t.check(await cand(d, 'sdl').locator('input').isChecked(), 'sdl is the one chosen')
  const steps = await d.locator('[data-testid=replace-steps]').innerText()
  t.check(/keeps serving.*never loses redundancy/s.test(steps) && /superblock is wiped/.test(steps), 'the steps: a live copy, then the old one released', steps)
  await t.shot(page, 'replace-raid')
  await d.locator('[data-testid=replace-next]').click()
  const word = d.locator('[data-testid=replace-word]')
  t.check(await t.until(async () => (await word.innerText()).trim().length > 0), 'the word to type is the engine guard’s', await word.innerText().catch(() => ''))
  await d.locator('input[aria-label=Confirmation]').fill('wrong')
  t.check(await d.locator('[data-testid=replace-go]').isDisabled(), 'the wrong word starts nothing')
  await d.locator('input[aria-label=Confirmation]').fill((await word.innerText()).trim())
  await d.locator('[data-testid=replace-go]').click()
  t.check(await t.until(async () => !(await d.isVisible())), 'the dialog closes once the engine has it', await page.locator('[data-testid=replace-error]').innerText().catch(() => ''))
  const rrow = (name) => page.locator(`[data-testid=replace-row][data-drive="${name}"]`)
  t.check(await t.until(async () => (await rrow('sdk').getAttribute('data-state')) === 'running', 12000), "sdk's row: copying", await rrow('sdk').innerText().catch(() => ''))
  t.check(await t.until(async () => /copying slot 0 of ds0-a/.test(await rrow('sdk').innerText()) && /\d+%/.test(await rrow('sdk').innerText()), 12000), 'with the slot and a percent', await rrow('sdk').innerText().catch(() => ''))
  await t.shot(page, 'replace-raid-running')

  // sde, a slab member: sdd joins the fleet, then sde is drained and leaves.
  d = await openReplace('sde')
  t.check(/Slab member/i.test(await d.locator('[data-testid=replace-kind]').innerText()), 'sde reads Slab member')
  await cand(d, 'sdd').locator('input').check()
  const st2 = await d.locator('[data-testid=replace-steps]').innerText()
  t.check(/sdd joins the fleet/.test(st2) && /stormblock#563/.test(st2) && /leaves the fleet/.test(st2), 'the steps: join, drain, leave', st2)
  await d.locator('[data-testid=replace-next]').click()
  t.check((await d.locator('[data-testid=replace-word]').innerText()).trim() === 'FLT005', "the word is sde's serial")
  await d.locator('input[aria-label=Confirmation]').fill('FLT005')
  await d.locator('[data-testid=replace-go]').click()
  t.check(await t.until(async () => !(await d.isVisible())), 'started', await page.locator('[data-testid=replace-error]').innerText().catch(() => ''))
  t.check(await t.until(async () => /draining sde/.test(await rrow('sde').innerText()), 15000), "sde's row: draining, moved and left", await rrow('sde').innerText().catch(() => ''))

  // sdb, free: nothing to move, recorded.
  d = await openReplace('sdb')
  t.check(/Free/i.test(await d.locator('[data-testid=replace-kind]').innerText()), 'sdb reads Free')
  await cand(d, 'sdc').locator('input').check()
  t.check(/stormdrive#127/.test(await d.locator('[data-testid=replace-steps]').innerText()), 'saying the record is the console’s for now')
  await d.locator('[data-testid=replace-next]').click()
  await d.locator('input[aria-label=Confirmation]').fill('NET002')
  await d.locator('[data-testid=replace-go]').click()
  t.check(await t.until(async () => (await rrow('sdb').getAttribute('data-state')) === 'done'), "sdb's row: done, recorded", await rrow('sdb').innerText().catch(() => ''))

  // Firmware: a set's two members together are refused; sdb and sdc take an uploaded image.
  await page.locator('[data-testid=select-mode]').click()
  await bay('sdk').click()
  await bay('sdm').click()
  await page.locator('[data-testid=firmware-selected]').click()
  let f = page.locator('[data-testid=firmware-dialog]')
  t.check(await t.until(async () => f.isVisible()), 'Update firmware… for sdk and sdm opens')
  t.check(/both in RAID set ds0-a: one at a time/.test(await f.locator('[data-testid=firmware-selection-why]').innerText()), 'and says one member of a set at a time', await f.innerText())
  t.check(await f.locator('[data-testid=firmware-next]').isDisabled(), 'and starts nothing')
  await f.getByRole('button', { name: 'Cancel' }).click()
  await page.locator('[data-testid=selection] button.link', { hasText: 'Clear' }).click()
  await bay('sdb').click()
  await bay('sdc').click()
  await page.locator('[data-testid=firmware-selected]').click()
  f = page.locator('[data-testid=firmware-dialog]')
  t.check(await t.until(async () => f.isVisible()), 'Update firmware… for sdb and sdc opens')
  t.check((await f.locator('[data-testid=firmware-version]').allInnerTexts()).every((v) => v === 'A3Z0'), 'both on A3Z0 now')
  const named = await f.locator('[data-testid=firmware-image]').evaluateAll((els) => els.map((e) => e.getAttribute('data-image')))
  // The image uploaded to storm-b's store is for their model, and is not in this node's: said so.
  const remote = f.locator('[data-testid=firmware-image][data-image="H7210A521SUN010T+A3Z2.bin"]')
  t.check(named.includes('loose-A3Z1.bin') && /names no model/.test(await f.innerText()), 'the image naming no model, said as such', named.join(', '))
  t.check(/not on /.test(await remote.innerText()), "storm-b's copy of their model's image: not in this node's store", await remote.innerText())
  await remote.locator('input').check()
  t.check(/not in the image store of/.test(await f.locator('[data-testid=firmware-image-why]').innerText()) && (await f.locator('[data-testid=firmware-next]').isDisabled()), 'chosen, it says to upload it here, and starts nothing')
  await f.locator('[data-testid=firmware-file]').setInputFiles({ name: 'A3Z2.bin', mimeType: 'application/octet-stream', buffer: Buffer.alloc(32768, 7) })
  t.check(/stored as H7210A521SUN010T\+A3Z2\.bin/.test(await f.innerText()), 'the upload is named for the model')
  await f.locator('[data-testid=firmware-upload]').click()
  t.check(await t.until(async () => /stored on/.test(await f.locator('[data-testid=firmware-said]').innerText())), 'uploaded', await f.locator('[data-testid=firmware-error]').innerText().catch(() => ''))
  const img = f.locator('[data-testid=firmware-image][data-image="H7210A521SUN010T+A3Z2.bin"] input')
  t.check(await t.until(async () => img.isChecked()), 'and chosen')
  await t.shot(page, 'firmware-dialog')
  await f.locator('[data-testid=firmware-next]').click()
  await f.locator('input[aria-label=Confirmation]').fill('update 2 drives')
  await f.locator('[data-testid=firmware-go]').click()
  t.check(await t.until(async () => !(await f.isVisible())), 'started', await page.locator('[data-testid=firmware-error]').innerText().catch(() => ''))
  const frow = (name) => page.locator(`[data-testid=firmware-row][data-drive="${name}"]`)
  t.check(await t.until(async () => /% sent/.test(await frow('sdb').innerText()), 15000), "sdb's row: the image going over", await frow('sdb').innerText().catch(() => ''))

  // sdd alone, whose drive refuses the download. Starting cleared the selection.
  t.check((await page.locator('[data-testid=selection]').count()) === 0, 'the selection is cleared once they start')
  await page.locator('[data-testid=select-mode]').click()
  await pick('sdd')
  m = await menu()
  await item(m, /^Update firmware…/).click()
  f = page.locator('[data-testid=firmware-dialog]')
  await f.locator('[data-testid=firmware-image][data-image="H7210A521SUN010T+A3Z2.bin"] input').check()
  await f.locator('[data-testid=firmware-next]').click()
  await f.locator('input[aria-label=Confirmation]').fill('NET004')
  await f.locator('[data-testid=firmware-go]').click()
  t.check(await t.until(async () => !(await f.isVisible())), 'sdd started')

  // Everything to its end, read back.
  t.check(await t.until(async () => (await rrow('sdk').getAttribute('data-state')) === 'done', RUN + 20000), "sdk's replace done", await rrow('sdk').innerText().catch(() => ''))
  t.check(/slot 0 holds sdl \(NEW012\), active; set clean/.test(await rrow('sdk').innerText()), 'read back from the engine: sdl holds the slot', await rrow('sdk').innerText())
  t.check(await t.until(async () => (await rrow('sde').getAttribute('data-state')) === 'done', RUN + 20000), "sde's replace done", await rrow('sde').innerText().catch(() => ''))
  t.check(await t.until(async () => /empty \(100 moved\) and out of the fleet: it can be pulled/.test(await rrow('sde').innerText()), 20000), 'read back from stormdrive: empty, then out of the fleet', await rrow('sde').innerText())
  t.check(await t.until(async () => (await frow('sdb').getAttribute('data-state')) === 'done', RUN + 20000), "sdb's update done", await frow('sdb').innerText().catch(() => ''))
  t.check(await t.until(async () => /reads back A3Z2 \(was A3Z0\)/.test(await frow('sdc').innerText()), 15000), 'sdc reads back A3Z2', await frow('sdc').innerText().catch(() => ''))
  t.check(await t.until(async () => (await frow('sdd').getAttribute('data-state')) === 'failed', RUN + 20000), "sdd's update failed", await frow('sdd').innerText().catch(() => ''))
  t.check(/still A3Z0: WRITE BUFFER/.test(await frow('sdd').innerText()), 'and says the drive is still on A3Z0, and why', await frow('sdd').innerText())
  await t.shot(page, 'maintain-done')

  // A reload keeps the rows: they are read again from the engine and stormdrive.
  await page.reload()
  t.check(await t.until(async () => (await rrow('sdk').count()) > 0 && (await frow('sdb').count()) > 0, 15000), 'after a reload the rows are still there')

  // bob, a storage-viewer: no firmware for him, and no set member's replace.
  const bobp = await t.page('bob')
  t.check(await t.open(bobp, '#/drives', /drives/i), 'bob opens Drives')
  await bobp.locator('button.bay[data-drive^="sdc "]').first().click()
  await bobp.locator('.picked [data-testid=drive-actions]').first().click()
  const bi = bobp.getByRole('menuitem', { name: /^Update firmware…/ })
  t.check(await bi.isDisabled(), "bob's Update firmware… is off", await bi.innerText())
  await bobp.keyboard.press('Escape')
  await bobp.locator('button.bay[data-drive^="sdm "]').first().click()
  await bobp.locator('.picked [data-testid=drive-actions]').first().click()
  await bobp.getByRole('menuitem', { name: /^Replace…/ }).click()
  const bd = bobp.locator('[data-testid=replace-dialog]')
  t.check(await t.until(async () => /storage-admin/.test(await bd.innerText())), "bob's replace of a set member says it needs storage-admin", await bd.innerText().catch(() => ''))
})
