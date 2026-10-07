// Shared by the page walks in deploy/browser (#58): a headless Chromium on
// a real console, failing on any page error or console error, with the
// small helpers every walk needs. A walk is `walk(async (t) => { ... })`.
//
// Console errors from a 4xx answer the page was built to show (a refusal,
// a 404 for "not there") are not page errors and are ignored; anything
// else the browser logs as an error is.
const { chromium } = require('playwright')

const BASE = process.env.CONSOLE
const SHOTS = process.env.SHOTS || '.'

exports.walk = (name, fn) => {
  ;(async () => {
    let failed = 0
    const errors = []
    const browser = await chromium.launch()
    const t = {
      BASE,
      check(ok, what, extra = '') {
        console.log(`  ${ok ? 'ok  ' : 'FAIL'} ${what}${extra ? ` — ${String(extra).replace(/\s+/g, ' ').slice(0, 300)}` : ''}`)
        if (!ok) failed++
        return ok
      },
      pause: (ms) => new Promise((r) => setTimeout(r, ms)),
      async until(fn, ms = 15000) {
        const end = Date.now() + ms
        while (Date.now() < end) {
          try {
            if (await fn()) return true
          } catch {}
          await t.pause(400)
        }
        return false
      },
      /// A page as `user` (signed in), or anonymous when `user` is empty.
      async page(user = '', password = 'pw') {
        const ctx = await browser.newContext({ viewport: { width: 1440, height: 1000 } })
        const page = await ctx.newPage()
        page.setDefaultTimeout(8000)
        const who = user || 'anonymous'
        page.on('pageerror', (e) => errors.push(`${who} pageerror: ${e.message.split('\n')[0]}`))
        page.on('console', (m) => {
          if (m.type() !== 'error') return
          if (/the server responded with a status of 4\d\d/.test(m.text())) return
          errors.push(`${who} console: ${m.text().split('\n')[0]}`)
        })
        if (user) {
          const r = await ctx.request.post(`${BASE}/api/v1/auth/login`, { data: { username: user, password } })
          t.check(r.ok(), `signed in as ${user}`, `${r.status()}`)
        }
        return page
      },
      /// Open `hash`, wait until the main area has text matching `re`.
      async open(page, hash, re, ms = 15000) {
        await page.goto(`${BASE}/${hash}`)
        const ok = await t.until(async () => re.test(await page.locator('main').innerText()), ms)
        return ok
      },
      text: (page) => page.locator('main').innerText(),
      shot: (page, n) => page.screenshot({ path: `${SHOTS}/${n}.png`, fullPage: true }).catch(() => {}),
    }
    console.log(`\n--- browser: ${name}`)
    try {
      await fn(t)
    } catch (e) {
      t.check(false, 'the walk ran to its end', e.message.split('\n')[0])
    }
    t.check(errors.length === 0, 'no page errors', errors.join(' | '))
    await browser.close()
    console.log(`--- browser: ${name}: ${failed} failed`)
    process.exit(failed ? 1 : 0)
  })()
}
