// The browser half of deploy/verify-storage-guard.sh (#82): alice, a
// storage-admin, opens a drive and presses Format 4K — asked OK, then asked
// to type the serial; a wrong word stops it, the serial does it. bob, a
// storage-viewer, opens the same drive and is offered Locate and no Format.
// Any page error fails the run.
const { chromium } = require('playwright')

const BASE = process.env.CONSOLE
let failed = 0
const errors = []
function check(ok, what, extra = '') {
  console.log(`  ${ok ? 'ok  ' : 'FAIL'} ${what}${extra ? ` — ${extra}` : ''}`)
  if (!ok) failed++
}
const pause = (ms) => new Promise((r) => setTimeout(r, ms))

async function as(browser, user) {
  const ctx = await browser.newContext()
  const page = await ctx.newPage()
  page.on('pageerror', (e) => errors.push(`${user} pageerror: ${e.message.split('\n')[0]}`))
  page.on('console', (m) => {
    if (m.type() === 'error' && !/the server responded with a status of 4(03|28)/.test(m.text()))
      errors.push(`${user} console: ${m.text().split('\n')[0]}`)
  })
  const login = await ctx.request.post(`${BASE}/api/v1/auth/login`, { data: { username: user, password: 'pw' } })
  check(login.ok(), `signed in as ${user}`, `${login.status()}`)
  const feed = await (await ctx.request.get(`${BASE}/api/v1/components`)).json()
  const drive = feed.find((c) => c.kind === 'drive' && c.label.startsWith('sdb'))
  return { ctx, page, drive }
}

async function openDrive(page, drive) {
  await page.goto(`${BASE}/#/grid?id=${encodeURIComponent(drive.id)}`)
  await page.waitForLoadState('networkidle')
  await page.getByRole('button', { name: 'Expand' }).first().click()
  await page.getByText('actions', { exact: true }).first().waitFor({ timeout: 5000 })
}
const button = (page, label) => page.locator('button:visible', { hasText: new RegExp(`^${label}$`) })

;(async () => {
  const browser = await chromium.launch()

  // alice
  {
    const { page, drive } = await as(browser, 'alice')
    check(!!drive, 'alice: the drive is in her feed', drive?.id)
    const prompts = []
    let word = 'sdb'
    page.on('dialog', async (d) => {
      if (d.type() === 'prompt') {
        prompts.push(d.message())
        await d.accept(word)
      } else {
        await d.accept()
      }
    })
    await openDrive(page, drive)
    check((await button(page, 'Format 4K').count()) > 0, 'alice is offered Format 4K')
    check((await button(page, 'Destructive test').count()) > 0, 'alice is offered Destructive test')

    await button(page, 'Format 4K').first().click()
    await page.getByText(/not confirmed: that is not ZC1234/).first().waitFor({ timeout: 8000 }).catch(() => {})
    check(prompts.length === 1, 'asked to type, after the OK', `${prompts.length} prompt(s)`)
    check(/ZC1234/.test(prompts[0] || '') && /Format a drive/.test(prompts[0] || ''), 'the prompt names the serial and what it does', JSON.stringify(prompts[0] || ''))
    check(await page.getByText(/not confirmed: that is not ZC1234/).count() > 0, 'typing the device name instead stops it, and says so')

    word = 'ZC1234'
    await button(page, 'Format 4K').first().click()
    const done = page.getByText(/POST \/api\/v1\/drives\/[0-9a-f-]+\/format\/4096 accepted/).first()
    await done.waitFor({ timeout: 8000 }).catch(() => {})
    check(prompts.length === 2, 'asked again')
    check(await done.count() > 0, 'typing ZC1234 formats it: stormdrive accepted')
    await page.screenshot({ path: 'alice.png' })
  }

  // bob
  {
    const { page, drive } = await as(browser, 'bob')
    check(!!drive, 'bob: the same drive is in his feed', drive?.id)
    let dialogs = 0
    page.on('dialog', async (d) => {
      dialogs++
      await d.dismiss()
    })
    await openDrive(page, drive)
    check((await button(page, 'Locate').count()) > 0, 'bob is offered Locate')
    check((await button(page, 'Format 4K').count()) === 0, 'bob is not offered Format 4K')
    check((await button(page, 'Destructive test').count()) === 0, 'bob is not offered Destructive test')
    check(await page.getByText('ZC1234').count() > 0, 'bob sees the drive in full (its serial)')
    await button(page, 'Locate').first().click()
    await pause(1500)
    check(dialogs === 0, 'Locate asks nothing')
  }

  await browser.close()
  for (const e of errors) console.log(`  FAIL ${e}`)
  failed += errors.length
  console.log(`browser: ${failed} failed`)
  process.exit(failed ? 1 : 0)
})().catch((e) => {
  console.error(e)
  process.exit(1)
})
