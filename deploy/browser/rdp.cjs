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
    t.check(await t.until(async () => /Could not connect|Session ended/.test(await box.innerText()), 40000), "the gateway's RBAC check refuses him", await box.innerText())
    t.check((await state.innerText()).toLowerCase() === 'idle', 'and no session is left')
    return
  }
  t.check(await t.until(async () => (await state.innerText()).toLowerCase() === 'connected', 40000), 'carol connects', await box.innerText())
  // The frame arrives after the session opens, and a canvas's size says nothing about what is on it.
  const painted = async () => page.evaluate(() => {
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
    for (let i = 0; i < d.length; i += 4) seen.add(`${d[i]},${d[i + 1]},${d[i + 2]}`)
    return seen.size
  })
  t.check(await t.until(async () => (await painted()) > 1, 40000), "the machine's screen is drawn: more than one colour", String(await painted()))
  await t.shot(page, 'rdp-connected')
  await page.keyboard.press('Tab').catch(() => {})
  await box.getByRole('button', { name: 'Disconnect' }).click()
  t.check(await t.until(async () => (await state.innerText()).toLowerCase() === 'idle', 20000), 'Disconnect ends it', await box.innerText())
})
