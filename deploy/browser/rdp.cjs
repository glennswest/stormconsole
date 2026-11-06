// RDP in a machine's page (#54), over deploy/verify-vm-rdp.sh: a real gateway, a real apiserver and a real framebuffer.
const { walk } = require('./lib.cjs')
const { PHASE } = process.env

walk(`rdp (${PHASE})`, async (t) => {
  const page = await t.page(PHASE)
  await page.setViewportSize({ width: 1440, height: 1000 })
  t.check(await t.open(page, '#/vm/shop/vm1', /Machine|State/), "the machine's page")
  await page.locator('nav.tabs button', { hasText: 'Remote desktop' }).click()
  const box = page.locator('[data-testid=rdp]')
  const connect = box.getByRole('button', { name: 'Connect' })
  if (PHASE === 'dan' || PHASE === 'erin') {
    const want = PHASE === 'dan' ? /kubernetes identity/ : /operator role/
    t.check(await t.until(async () => want.test(await box.innerText())), 'told why', await box.innerText().catch(() => ''))
    t.check(await connect.isDisabled(), 'and cannot connect')
    return
  }
  t.check(await t.until(() => connect.isEnabled()), 'Connect is offered', await box.innerText().catch(() => ''))
  await connect.click()
  const state = box.locator('.state')
  if (PHASE === 'dave') {
    t.check(await t.until(async () => /The gateway refused you/.test(await box.innerText()), 40000), "the gateway's RBAC check refuses him", await box.innerText())
    t.check((await state.innerText()).toLowerCase() === 'idle', 'and no session is left')
    return
  }
  t.check(await t.until(async () => (await state.innerText()).toLowerCase() === 'connected', 40000), 'carol connects', await box.innerText())
  // The frame arrives after the session opens, and a canvas's size says nothing about what is on it.
  const canvas = async () => page.evaluate(() => {
    const find = (root) => {
      for (const e of root.querySelectorAll('*')) {
        if (e.tagName === 'CANVAS') return e
        if (e.shadowRoot) { const c = find(e.shadowRoot); if (c) return c }
      }
      return null
    }
    const c = find(document)
    if (!c || !c.width) return 0
    const off = document.createElement('canvas'); off.width = 64; off.height = 48
    const g = off.getContext('2d'); g.drawImage(c, 0, 0, 64, 48)
    const d = g.getImageData(0, 0, 64, 48).data
    const seen = new Set()
    let sum = 0
    for (let i = 0; i < d.length; i += 4) { seen.add(`${d[i]},${d[i + 1]},${d[i + 2]}`); sum = (sum * 31 + d[i]) >>> 0 }
    return { colours: seen.size, sum }
  })
  const painted = async () => ((await canvas()) || { colours: 0 }).colours
  t.check(await t.until(async () => (await painted()) > 1, 40000), "the machine's screen is drawn: more than one colour", String(await painted()))
  await t.shot(page, 'rdp-connected')
  // This guest has no disk and stops at "No bootable device", so its screen holds still until something reaches it.
  const still = (await canvas()).sum
  await page.waitForTimeout(3000)
  t.check((await canvas()).sum === still, 'the halted guest draws nothing by itself')
  await box.getByRole('button', { name: 'Ctrl+Alt+Del' }).click()
  t.check(await t.until(async () => (await canvas()).sum !== still, 20000), 'Ctrl+Alt+Del reaches the guest: it reboots')
  await box.getByRole('button', { name: 'Disconnect' }).click()
  t.check(await t.until(async () => /Disconnected\./.test(await box.innerText()), 20000), 'Disconnect says so, not a failure', await box.innerText())
  t.check((await state.innerText()).toLowerCase() === 'idle', 'and it is over')
  await connect.click()
  t.check(await t.until(async () => (await state.innerText()).toLowerCase() === 'connected', 40000), 'and it connects again, with a new login', await box.innerText())
  t.check(await t.until(async () => (await painted()) > 1, 40000), 'drawn again')
  await box.getByRole('button', { name: 'Disconnect' }).click()
  await t.until(async () => /Disconnected\./.test(await box.innerText()), 20000)
})
