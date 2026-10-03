// The words half of deploy/verify-pod-page.sh (#70): every page that shows
// a registry image or an instance, read as a person reads it — the visible
// text, every tooltip, the navigator, the Create menu and an opened row —
// searched for "golden". The stand-ins name nothing with the word, so a
// hit is the console's wording. The one allowed: the pod page's Registry
// image tooltip, which names the API's word on purpose.
const { chromium } = require('playwright')

const BASE = process.env.CONSOLE
const SHOTS = process.env.SHOTS
let failed = 0
const errors = []
function check(ok, what, extra = '') {
  console.log(`  ${ok ? 'ok  ' : 'FAIL'} ${what}${extra ? ` — ${extra}` : ''}`)
  if (!ok) failed++
}
const pause = (ms) => new Promise((r) => setTimeout(r, ms))

;(async () => {
  const browser = await chromium.launch()
  const page = await browser.newPage({ viewport: { width: 1440, height: 1000 } })
  page.on('pageerror', (e) => errors.push(`pageerror: ${e.message.split('\n')[0]}`))

  // Visible text and every title/aria-label/placeholder/option on the page.
  const words = () =>
    page.evaluate(() => {
      const out = [document.body.innerText]
      for (const el of document.querySelectorAll('[title],[aria-label],[placeholder]')) {
        for (const a of ['title', 'aria-label', 'placeholder']) {
          const v = el.getAttribute(a)
          if (v) out.push(`@${a}: ${v}`)
        }
      }
      for (const o of document.querySelectorAll('option')) out.push(`option: ${o.textContent}`)
      return out.join('\n')
    })
  const hits = (text) => text.split('\n').filter((l) => /golden/i.test(l))

  const tooltip = '@title: registry image (golden) — “golden” is the API’s name for it'
  const allowed = (l) => l === tooltip

  async function visit(hash, what, expect = [], act = null) {
    await page.goto(`${BASE}/${hash}`)
    await pause(2500)
    if (act) await act()
    const text = await words()
    const bad = hits(text).filter((l) => !allowed(l))
    check(bad.length === 0, `${what}: no "golden"`, bad.slice(0, 4).join(' | '))
    for (const e of expect) check(text.includes(e), `${what}: says “${e}”`)
    await page.screenshot({ path: `${SHOTS}/words-${what.replace(/\W+/g, '-')}.png`, fullPage: true })
    return text
  }

  // Open every navigator section so its items are visible text.
  const openNav = async () => {
    for (const b of await page.locator('nav button[aria-expanded="false"]').all()) await b.click().catch(() => {})
    await pause(300)
  }

  await visit('#/', 'overview', ['Registry images'], openNav)
  await visit('#/images', 'images page', ['Registry images', 'Container registry images', 'Component registry images', 'Slab registry images'])
  await visit('#/grid?id=reg:registry&rel=goldens', 'registry relation', ['Registry images', 'nats-2-10-sealed'])
  await visit('#/grid?id=reg:registry&rel=clones', 'instances', ['clone of nats-2-10-sealed'])
  await visit('#/grid?id=img:operator&rel=goldens', 'vm registry images', ['Registry images'])
  await visit('#/grid?id=img:operator&rel=catalogue', 'vm catalogue', ['Delete registry image', 'Make registry image'], async () => {
    // Open every row, so the actions and references are on the page.
    for (const b of await page.locator('button[aria-label^="Expand"], button.exp, td.ctl button').all()) await b.click().catch(() => {})
    await pause(400)
  })
  await visit('#/grid?id=sb:engine&rel=volumes', 'volumes', [], async () => {
    for (const b of await page.locator('td.ctl button').all()) await b.click().catch(() => {})
    await pause(400)
  })
  await visit('#/grid?id=sb:engine', 'engine', [])
  // A card links a relation with the word it shows; it must still resolve.
  await visit('#/grid?id=reg:registry&rel=registry%20images', 'translated rel link', ['nats-2-10-sealed'])

  // Cards: the same lists as stormview's card renders them.
  await page.goto(`${BASE}/#/grid?id=reg:registry`)
  await pause(2000)
  const cardText = await words()
  check(hits(cardText).filter((l) => !allowed(l)).length === 0, 'registry card: no "golden"', hits(cardText).slice(0, 3).join(' | '))

  // The Create menu, everywhere it is offered.
  await page.goto(`${BASE}/#/images`)
  await pause(1500)
  await page.getByRole('button', { name: /Create/ }).first().click().catch(() => {})
  await pause(500)
  const menu = await words()
  check(hits(menu).filter((l) => !allowed(l)).length === 0, 'Create menu: no "golden"', hits(menu).slice(0, 3).join(' | '))
  check(/Registry image/.test(menu) && /Instance/.test(menu), 'Create menu offers Registry image and Instance')
  await page.screenshot({ path: `${SHOTS}/words-create-menu.png`, fullPage: true })

  // The pod page: the Registry image row, and the one tooltip.
  const pod = await visit('#/pod/shop/web-1', 'pod page', ['Registry image', 'cilium@abc123def456', 'sealed digest sha256:', 'from cilium@abc123'])
  check(pod.split('\n').filter(allowed).length === 1, 'the API word appears once, in the tooltip')

  for (const e of errors) console.log(`  ${e}`)
  check(errors.length === 0, 'no page errors', `${errors.length}`)
  await browser.close()
  console.log(failed ? `words: ${failed} failed` : 'words: all ok')
  process.exit(failed ? 1 : 0)
})().catch((e) => {
  console.log(`FAIL ${e.message}`)
  process.exit(1)
})
