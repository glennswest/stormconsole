// The format page and the Drives page in a browser (#165, #308), over
// deploy/verify-drive-format.sh. This node is laid out as forge1 is: sda the
// system drive, sdb-sdd free, planned 4096 + PI type 1.
// PHASE=start: who is offered what, the dialog, three drives at 4096 + PI 1,
//   progress per drive; the Drives page: in-use shown, a selection formatted,
//   the Actions menu at 1280 and 390 px.
// PHASE=follow: after a console restart, following them to the read-back.
// PHASE=nologin: forge1's console, with no sign-in.
const { walk } = require('./lib.cjs')

walk(`drive format, ${process.env.PHASE}`, async (t) => {
  const main = (p) => t.text(p)
  const row = (p, name) => p.locator('table.drives tr', { hasText: name }).first()
  const dialogRow = (d, name) => d.locator(`[data-testid=dialog-drive][data-drive="${name}"]`)

  /// The Actions menu of the picked drive fits the window, groups its
  /// commands with the destructive ones last, and runs Locate.
  async function actionsMenu(page, width) {
    await page.setViewportSize({ width, height: 900 })
    t.check(await t.open(page, '#/drives', /drives/i), `Drives at ${width} px`)
    const bay = page.locator('button.bay[data-drive^="sdb "]').first()
    t.check(await t.until(async () => (await bay.count()) > 0), `sdb's bay on the map at ${width} px`)
    const trigger = page.locator('.picked [data-testid=drive-actions]').first()
    // Picked already (the page stays open between widths): a second click would let it go.
    const header = page.locator('.picked header').first()
    if (!((await trigger.isVisible().catch(() => false)) && /^sdb /.test(await header.innerText().catch(() => '')))) await bay.click()
    t.check(await t.until(async () => /^sdb /.test(await header.innerText())), `sdb picked at ${width} px`)
    t.check(await t.until(async () => trigger.isVisible()), `the picked drive has an Actions menu at ${width} px`)
    const tb = await trigger.boundingBox()
    t.check(tb && tb.x >= 0 && tb.x + tb.width <= width, `and the button is on the screen at ${width} px`, JSON.stringify(tb))
    await trigger.click()
    const menu = page.locator('[role=menu]').first()
    t.check(await t.until(async () => menu.isVisible()), 'the menu opens')
    const mb = await menu.boundingBox()
    t.check(mb && mb.x >= 0 && mb.x + mb.width <= width + 1, `inside the window at ${width} px`, JSON.stringify(mb))
    const names = await menu.locator('.gname').allInnerTexts()
    t.check(/destroys data/i.test(names.at(-1) || '') && names.slice(0, -1).every((n) => !/destroy/i.test(n)), 'grouped, the destructive ones last and marked', names.join(' | '))
    const items = await menu.locator('[role=menuitem]').allInnerTexts()
    if (!items.length) console.log('    (menus open:', await page.locator('[role=menu]').count(), ')')
    t.check(items.some((i) => /^Format…/.test(i)) && !items.some((i) => /Format 4K/.test(i)), 'Format… is there, the feed’s Format 4K is not', items.join(' | '))
    for (const want of [/^Locate/, /Smoke test/, /Join fleet/, /Mark spare/, /Destructive test/]) t.check(items.some((i) => want.test(i)), `it has ${want.source}`)
    await t.shot(page, `drives-actions-${width}`)
    // The page itself does not run past the window either.
    const sw = await page.evaluate(() => document.documentElement.scrollWidth)
    const wide = sw > width + 1 ? await page.evaluate((w) => [...document.querySelectorAll('body *')].filter((e) => e.getBoundingClientRect().right > w + 1 && !e.closest('[style*=overflow], .wrap')).slice(0, 8).map((e) => `${e.tagName.toLowerCase()}.${[...e.classList].join('.')} ${Math.round(e.getBoundingClientRect().right)}`).join(', '), width) : ''
    t.check(sw <= width + 1, `the page is no wider than the window at ${width} px`, `${sw} ${wide}`)
    await menu.getByRole('menuitem', { name: /^Locate/ }).click()
    t.check(await t.until(async () => /locate on/i.test(await main(page))), 'Locate from the menu ran', (await main(page)).slice(0, 200))
    await page.setViewportSize({ width: 1440, height: 1000 })
  }

  if (process.env.PHASE === 'nologin') {
    // forge1's case: authentication off, nobody signed in (#165).
    const p = await t.page('')
    t.check(await t.open(p, '#/drives/format', /NET003/), 'no sign-in: the page opens', (await t.text(p)).slice(0, 200))
    t.check((await p.locator('[data-testid=refused]').count()) === 0, 'with no refusal')
    await row(p, 'sdc').locator('input[type=checkbox]').check()
    await p.locator('[data-testid=open-dialog]').click()
    const dialog = p.locator('[role=dialog]')
    await dialog.locator('[data-testid=dialog-next]').click()
    await dialog.locator('input[aria-label=Confirmation]').fill('NET003')
    await dialog.locator('[data-testid=dialog-format]').click()
    const running = p.locator('[data-testid=running]')
    t.check(await t.until(async () => /sdc/.test(await running.innerText())), 'and a format starts, and shows', await running.innerText().catch(() => ''))
    await t.shot(p, 'format-nologin')
    return
  }

  if (process.env.PHASE === 'start') {
    // bob reads and is offered nothing; dana is a console admin with no identity.
    const bob = await t.page('bob')
    t.check(await t.open(bob, '#/drives/format', /NET002/), 'bob sees the drives')
    t.check((await bob.locator('table.drives input[type=checkbox]').count()) === 0 && (await bob.locator('[data-testid=open-dialog]').isDisabled()), 'and has no way to start a format')
    t.check(/not start one/.test(await bob.locator('[data-testid=refused]').innerText()), 'and is told', await bob.locator('[data-testid=refused]').innerText())
    const dana = await t.page('dana')
    await t.open(dana, '#/drives/format', /NET002/)
    t.check(/no kubernetes identity/.test(await dana.locator('[data-testid=refused]').innerText()), 'a console admin with no identity is told why', await dana.locator('[data-testid=refused]').innerText())

    const page = await t.page('alice')
    t.check(await t.open(page, '#/drives/format', /NET002/), 'alice opens the page')
    t.check(await t.until(async () => (await page.locator('table.drives input[type=checkbox]').count()) > 0), 'alice may choose drives')
    const nav = await page.evaluate(() => fetch('/api/v1/console/nav').then((r) => r.json()))
    t.check(/#\/drives\/format/.test(JSON.stringify(nav)), 'Hardware has a Format item')

    // forge1's sda: System, why, and not selectable.
    const sda = await row(page, 'sda').innerText()
    t.check(/system/i.test(sda) && /system slab/.test(sda) && /never formatted from the console/.test(sda), 'sda reads System, with why it is never formatted', sda)
    t.check(await row(page, 'sda').locator('input[type=checkbox]').isDisabled(), 'and cannot be ticked')
    const sdb = await row(page, 'sdb').innerText()
    t.check(/H7210A521SUN010T/.test(sdb) && /NET002/.test(sdb) && /naa\.d2/.test(sdb) && /512 \+ PI type 1/.test(sdb) && /free/.test(sdb), 'sdb: model, serial, WWN, now 512 + PI 1, free', sdb)
    t.check(/4096 \+ T10 PI type 1/.test(sdb) && /if refused: 4096, no PI, then 512 \+ PI type 1/.test(sdb), 'and its plan with the fallbacks', sdb)
    for (const [name, why] of [['sde', /in the storage fleet/], ['sdf', /holds data for LVM2_member/]]) {
      const r = row(page, name)
      t.check(why.test(await r.innerText()), `${name} is refused, saying why`, await r.innerText())
      t.check(await r.locator('input[type=checkbox]').isDisabled(), 'and cannot be chosen')
    }
    t.check(/not probed yet/.test(await row(page, 'sdg').innerText()), 'a drive not probed says so')
    await t.shot(page, 'format-choose')

    // The dialog, from the page button with nothing ticked.
    await page.locator('[data-testid=open-dialog]').click()
    const dialog = page.locator('[role=dialog]')
    t.check(await t.until(async () => (await dialog.locator('[data-testid=dialog-drive]').count()) >= 9), 'the dialog lists every drive')
    const dsda = dialogRow(dialog, 'sda')
    t.check(/system/i.test(await dsda.innerText()) && /never formatted/.test(await dsda.innerText()), 'sda in the dialog: shown, System, the reason', await dsda.innerText())
    t.check(await dsda.locator('input[type=checkbox]').isDisabled(), 'and it cannot be picked')
    for (const n of ['sdb', 'sdc', 'sdd']) {
      const r = dialogRow(dialog, n)
      await r.locator('input[type=checkbox]').check()
      const sel = r.locator('select')
      const opts = await sel.locator('option').allInnerTexts()
      t.check(opts.length === 4 && /4096 \+ T10 PI type 1/.test(opts[0]) && opts.every((o) => !/type 2/.test(o)), `${n}: block size and PI offered, what stormdrive formats`, opts.join(' | '))
      t.check(/4096 \+ T10 PI type 1/.test(await sel.locator('option:checked').innerText()), `${n}: its plan by default`)
      t.check(/PI type 2; stormdrive formats type 1 or none/.test(await r.innerText()), `${n}: type 2 said, not offered`)
    }
    t.check(/3 chosen/.test(await dialog.innerText()), '3 chosen')
    await t.shot(page, 'format-dialog')
    await dialog.locator('[data-testid=dialog-next]').click()
    const lines = await dialog.locator('[data-testid=dialog-lines]').innerText()
    for (const n of ['sdb', 'sdc', 'sdd']) t.check(new RegExp(`${n} → 4096 \\+ T10 PI type 1`).test(lines), `the confirm names ${n} → 4096 + T10 PI type 1`, lines)
    t.check(!/sda/.test(lines), 'and not sda')
    const go = dialog.locator('[data-testid=dialog-format]')
    t.check(await go.isDisabled(), 'Format cannot be pressed yet')
    await dialog.locator('input[aria-label=Confirmation]').fill('format 2 drives')
    t.check(await go.isDisabled(), 'nor with the wrong words')
    await dialog.locator('input[aria-label=Confirmation]').fill('format 3 drives')
    await t.shot(page, 'format-confirm')
    await go.click()

    // Progress per drive.
    const running = page.locator('[data-testid=running]')
    const prow = (n) => running.locator(`[data-testid=progress-row][data-drive="${n}"]`)
    t.check(await t.until(async () => (await running.locator('[data-testid=progress-row]').count()) === 3), 'a row each for the three, at once')
    t.check(await t.until(async () => /prepare|formatting/.test(await prow('sdb').locator('[data-testid=progress-state]').innerText()) && /\d+%/.test(await prow('sdb').innerText()) && /elapsed/.test(await prow('sdb').innerText())), 'per drive: its phase, percent, elapsed', await prow('sdb').innerText().catch(() => ''))
    t.check(await t.until(async () => /formatting/.test(await prow('sdc').locator('[data-testid=progress-state]').innerText()) && /left/.test(await prow('sdc').innerText())), 'then formatting, with time left', await prow('sdc').innerText().catch(() => ''))
    t.check(/Controller reset: host3/.test(await prow('sdb').innerText()), 'a controller reset is said on its drive', await prow('sdb').innerText())
    await t.shot(page, 'format-running')
    await page.reload()
    t.check(await t.until(async () => (await page.locator('[data-testid=running] [data-testid=progress-row]').count()) === 3), 'a reload shows the three again')
    t.check(await row(page, 'sdb').locator('input[type=checkbox]').isDisabled(), 'a drive formatting cannot be chosen again')

    // The Drives page: in use at a glance, a selection, its progress.
    t.check(await t.open(page, '#/drives', /drives/i), 'the Drives page')
    const sdaBay = page.locator('button.bay[data-drive^="sda "]').first()
    t.check(await t.until(async () => (await sdaBay.count()) > 0), 'sda on the map')
    t.check(await t.until(async () => /in use: System/.test(await sdaBay.getAttribute('title')) && (await sdaBay.getAttribute('class')).includes('inuse')), 'sda marked in use on the map, System', await sdaBay.getAttribute('title'))
    t.check(!(await page.locator('button.bay[data-drive^="sdh "]').getAttribute('class')).includes('inuse'), 'sdh is not')
    t.check(/in use/.test(await page.locator('.legend').innerText()), 'the legend says what the mark means')
    await sdaBay.click()
    const picked = page.locator('.picked', { has: page.locator('[data-testid=picked-reason]') })
    t.check(await t.until(async () => /system slab/.test(await picked.innerText())), 'the picked sda says what holds it', await picked.innerText().catch(() => ''))
    t.check(/root/.test(await picked.innerText()) && /stormblock-data/.test(await picked.innerText()), 'what depends on it, and its slabs', await picked.innerText())
    t.check((await picked.locator('[data-testid=picked-format]').count()) === 0, 'no Format button on sda')
    await picked.locator('[data-testid=drive-actions]').click()
    const fmtItem = page.locator('[role=menu] [role=menuitem]', { hasText: 'Format…' })
    t.check(await fmtItem.isDisabled() && /never formatted/.test(await fmtItem.innerText()), 'its Format… is off, with the reason', await fmtItem.innerText())
    await page.keyboard.press('Escape')
    await t.shot(page, 'drives-sda')
    await page.locator('select[aria-label=Filter]').selectOption('inuse')
    t.check(await t.until(async () => (await page.locator('button.bay').count()) === 3), 'the In use filter: sda, the fleet drive, the LVM one', await page.locator('button.bay').count())
    await page.locator('select[aria-label=Filter]').selectOption('')
    await page.locator('.seg button', { hasText: 'List' }).click()
    t.check(await t.until(async () => /SYSTEM/i.test(await page.locator('tr', { hasText: 'sda ·' }).first().innerText())), 'the list says System on sda', await page.locator('tr', { hasText: 'sda ·' }).first().innerText().catch(() => ''))
    await page.locator('.seg button', { hasText: 'Map' }).click()

    await page.locator('[data-testid=select-mode]').click()
    for (const n of ['sdh', 'sdi']) await page.locator(`button.bay[data-drive^="${n} "]`).click()
    const bar = page.locator('[data-testid=selection]')
    t.check(/2 selected/.test(await bar.innerText()), 'two selected on the map', await bar.innerText())
    await bar.locator('[data-testid=format-selected]').click()
    const d2 = page.locator('[role=dialog]')
    t.check(await t.until(async () => (await d2.locator('input[type=checkbox]:checked').count()) === 2), 'the dialog opens on them')
    await d2.locator('[data-testid=dialog-next]').click()
    t.check(/sdh → 4096 \+ T10 PI type 1/.test(await d2.innerText()) && /sdi → 4096 \+ T10 PI type 1/.test(await d2.innerText()), 'each named with its result')
    await d2.locator('input[aria-label=Confirmation]').fill('format 2 drives')
    await d2.locator('[data-testid=dialog-format]').click()
    const dp = page.locator('[data-testid=drives-progress]')
    t.check(await t.until(async () => /sdh/.test(await dp.innerText()) && /sdi/.test(await dp.innerText()) && /sdb/.test(await dp.innerText())), 'the Drives page shows every run, per drive', await dp.innerText().catch(() => ''))
    await page.locator('[data-testid=select-mode]').click()
    await t.shot(page, 'drives-progress')

    await actionsMenu(page, 1280)
    await actionsMenu(page, 390)
    return
  }

  const page = await t.page('alice')
  t.check(await t.open(page, '#/drives/format', /Formatting now/), 'after the console restarted, the runs are still there')
  const running = page.locator('[data-testid=running]')
  t.check(/\d+%/.test(await running.innerText()), 'with their progress', await running.innerText())
  t.check(await t.until(async () => /verify/.test(await running.locator('[data-testid=progress-state]').allInnerTexts().then((x) => x.join(' '))), 60000), 'a run reaches the read-back (verify)')
  const ended = page.locator('[data-testid=ended]')
  t.check(await t.until(async () => {
    const x = await ended.innerText()
    return ['sdb', 'sdc', 'sdd', 'sdh', 'sdi'].every((n) => x.includes(n))
  }, 90000), 'they end, and stay on the page')
  const line = (n) => ended.locator('tr', { hasText: n }).innerText()
  for (const n of ['sdb', 'sdc', 'sdd']) t.check(/done/.test(await line(n)) && /4096 \+ PI type 1/.test(await line(n)), `${n}: done, read back 4096 + PI type 1`, await line(n))
  t.check(/done/.test(await line('sdh')) && /4096, no PI/.test(await line('sdh')) && /refused:/.test(await line('sdh')), 'a fallback: read back 4096, no PI, with the refusal', await line('sdh'))
  t.check(/failed/.test(await line('sdi')) && /512, no PI/.test(await line('sdi')) && /not what was asked/.test(await line('sdi')), 'a run that reads back otherwise: failed, saying what it reads', await line('sdi'))
  await t.shot(page, 'format-ended')

  // History.
  await row(page, 'sdb').getByRole('button', { name: 'History' }).click()
  const h = page.locator('[data-testid=history]')
  t.check(await t.until(async () => /alice/.test(await h.innerText()) && /root/.test(await h.innerText())), 'History: every run of the drive, and who asked', await h.innerText())
  await t.shot(page, 'format-history')

  // One drive on another node: its serial is the word.
  await row(page, 'sdx').locator('input[type=checkbox]').check()
  await page.locator('[data-testid=open-dialog]').click()
  const dialog = page.locator('[role=dialog]')
  await dialog.locator('[data-testid=dialog-next]').click()
  t.check(/Type BEE001 to confirm/i.test(await dialog.innerText()), 'one drive: type its serial', await dialog.innerText())
  await dialog.locator('input[aria-label=Confirmation]').fill('BEE001')
  await dialog.locator('[data-testid=dialog-format]').click()
  t.check(await t.until(async () => /storm-b · sdx/.test(await page.locator('[data-testid=running]').innerText())), 'another node formats through the same page')
})
