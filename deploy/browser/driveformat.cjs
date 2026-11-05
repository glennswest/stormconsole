// The format page in a browser (#165), over deploy/verify-drive-format.sh.
// PHASE=start: who is offered what, the confirmation, starting a format.
// PHASE=follow: after a console restart, following it to the read-back.
const { walk } = require('./lib.cjs')

walk(`drive format, ${process.env.PHASE}`, async (t) => {
  const main = (p) => t.text(p)
  const row = (p, name) => p.locator('table.drives tr', { hasText: name }).first()

  if (process.env.PHASE === 'start') {
    // bob reads and is offered nothing; dana is a console admin with no identity.
    const bob = await t.page('bob')
    t.check(await t.open(bob, '#/drives/format', /NET002/), 'bob sees the drives')
    t.check((await bob.locator('button.danger').count()) === 0 && (await bob.locator('table.drives input[type=checkbox]').count()) === 0, 'and has no way to start a format')
    t.check(/not start one/.test(await bob.locator('[data-testid=refused]').innerText()), 'and is told', await bob.locator('[data-testid=refused]').innerText())
    const dana = await t.page('dana')
    await t.open(dana, '#/drives/format', /NET002/)
    t.check(/no kubernetes identity/.test(await dana.locator('[data-testid=refused]').innerText()), 'a console admin with no identity is told why', await dana.locator('[data-testid=refused]').innerText())

    const page = await t.page('alice')
    t.check(await t.open(page, '#/drives/format', /NET002/), 'alice opens the page')
    t.check(await t.until(async () => (await page.locator('table.drives input[type=checkbox]').count()) > 0), 'alice may choose drives')
    const nav = await page.evaluate(() => fetch('/api/v1/console/nav').then((r) => r.json()))
    t.check(/#\/drives\/format/.test(JSON.stringify(nav)), 'Hardware has a Format item')

    // Each drive: what it is, and refused ones say why.
    const sdb = await row(page, 'sdb').innerText()
    t.check(/H7210A521SUN010T/.test(sdb) && /NET002/.test(sdb) && /naa\.d2/.test(sdb) && /520, no PI/.test(sdb), 'a row has model, serial, WWN and the current format', sdb)
    t.check(/4096 \+ PI type 1/.test(sdb) && /if refused: 4096, no PI, then 512 \+ PI type 1/.test(sdb), 'and the plan with its fallbacks', sdb)
    for (const [name, why] of [['sda', /designated system/], ['sde', /in the fleet/], ['sdf', /holds data for LVM2_member/]]) {
      const r = row(page, name)
      t.check(why.test(await r.innerText()), `${name} is refused, saying why`, await r.innerText())
      t.check(await r.locator('input[type=checkbox]').isDisabled(), `and cannot be chosen`)
    }
    t.check(/holds 1 slab/.test(await row(page, 'sde').innerText()), 'a drive with a slab says so')
    t.check(/not probed yet/.test(await row(page, 'sdg').innerText()), 'a drive not probed says so')
    await t.shot(page, 'format-choose')

    // Off the plan: exact, so no fallback.
    await page.locator('select[aria-label=Protection]').selectOption('none')
    t.check(!/if refused/.test(await row(page, 'sdb').innerText()), 'with PI off, a format off the plan has no fallbacks', await row(page, 'sdb').innerText())
    await page.locator('select[aria-label=Protection]').selectOption('offered')

    // Choose three; the confirmation names them.
    for (const n of ['sdb', 'sdc', 'sdd']) await row(page, n).locator('input[type=checkbox]').check()
    await page.locator('.choose button.danger').click()
    const dialog = page.locator('[role=dialog]')
    const d = await dialog.innerText()
    t.check(/their data goes/i.test(d) && /NET002/.test(d) && /NET003/.test(d) && /NET004/.test(d), 'the confirmation names the drives and that their data goes', d)
    const go = dialog.getByRole('button', { name: 'Format', exact: true })
    t.check(await go.isDisabled(), 'and cannot be pressed yet')
    await dialog.locator('input[aria-label=Confirmation]').fill('format 2 drives')
    t.check(await go.isDisabled(), 'nor with the wrong words')
    await dialog.locator('input[aria-label=Confirmation]').fill('format 3 drives')
    await t.shot(page, 'format-confirm')
    await go.click()
    const running = page.locator('[data-testid=running]')
    t.check(await t.until(async () => /sdb/.test(await running.innerText())), 'the run shows at once', await running.innerText().catch(() => ''))
    t.check(await t.until(async () => /\d+%/.test(await running.innerText()) && /elapsed/.test(await running.innerText()) && /left/.test(await running.innerText())), 'per drive: percent, elapsed, time left', await running.innerText())
    t.check(/Controller reset: host3/.test(await running.innerText()), 'a controller reset is said', await running.innerText())
    t.check(/refused a format: 4096 \+ type1/.test(await running.innerText()), 'a refused format is said, and what it runs instead', await running.innerText())
    await t.shot(page, 'format-running')
    await page.reload()
    t.check(await t.until(async () => /sdb/.test(await main(page)) && /elapsed/.test(await main(page))), 'a reload shows the run')
    t.check(await row(page, 'sdb').locator('input[type=checkbox]').isDisabled(), 'a drive formatting cannot be chosen again')
    return
  }

  const page = await t.page('alice')
  t.check(await t.open(page, '#/drives/format', /Formatting now/), 'after the console restarted, the run is still there')
  const running = page.locator('[data-testid=running]')
  t.check(/\d+%/.test(await running.innerText()), 'with its progress', await running.innerText())
  const ended = page.locator('[data-testid=ended]')
  t.check(await t.until(async () => /sdd/.test(await ended.innerText()) && /sdb/.test(await ended.innerText()), 60000), 'it ends, and stays on the page')
  const line = (n) => ended.locator('tr', { hasText: n }).innerText()
  t.check(/done/.test(await line('sdb')) && /4096 \+ PI type 1/.test(await line('sdb')), 'the result read back: 4096 + PI type 1', await line('sdb'))
  t.check(/done/.test(await line('sdc')) && /4096, no PI/.test(await line('sdc')) && /refused:/.test(await line('sdc')), 'the fallback: read back 4096, no PI, with the refusal', await line('sdc'))
  t.check(/failed/.test(await line('sdd')) && /512, no PI/.test(await line('sdd')) && /not what was asked/.test(await line('sdd')), 'a run that reads back otherwise: failed, saying what it reads', await line('sdd'))
  await t.shot(page, 'format-ended')

  // History.
  await row(page, 'sdb').getByRole('button', { name: 'History' }).click()
  const h = page.locator('[data-testid=history]')
  t.check(await t.until(async () => /alice/.test(await h.innerText()) && /root/.test(await h.innerText())), 'History: every run of the drive, and who asked', await h.innerText())
  await t.shot(page, 'format-history')

  // One drive on another node: its serial is the word.
  await row(page, 'sdx').locator('input[type=checkbox]').check()
  await page.locator('.choose button.danger').click()
  const dialog = page.locator('[role=dialog]')
  t.check(/Type BEE001 to confirm/i.test(await dialog.innerText()), 'one drive: type its serial', await dialog.innerText())
  await dialog.locator('input[aria-label=Confirmation]').fill('BEE001')
  await dialog.getByRole('button', { name: 'Format', exact: true }).click()
  t.check(await t.until(async () => /storm-b · sdx/.test(await page.locator('[data-testid=running]').innerText())), 'another node formats through the same page')
})
