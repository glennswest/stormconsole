// The browser half of deploy/verify-vm-network-edit.sh (#50): the owner's
// test1 edit, done as a person does it — Settings, Network, Change, type a
// bridge, Save — and what the page says afterwards. Any page error fails.
const { chromium } = require('playwright')

const BASE = process.env.CONSOLE
let failed = 0
const errors = []
function check(ok, what, extra = '') {
  console.log(`  ${ok ? 'ok  ' : 'FAIL'} ${what}${extra ? ` — ${extra}` : ''}`)
  if (!ok) failed++
}

;(async () => {
  const browser = await chromium.launch()
  const page = await browser.newPage({ viewport: { width: 1440, height: 1000 } })
  page.on('pageerror', (e) => errors.push(`pageerror: ${e.message.split('\n')[0]}`))
  page.on('console', (m) => {
    if (m.type() === 'error') errors.push(`console: ${m.text().split('\n')[0]}`)
  })

  await page.goto(`${BASE}/#/vm/default/test1`)
  await page.locator('nav.tabs button', { hasText: 'Settings' }).click()
  const row = page.locator('table.settings tr', { has: page.locator('th', { hasText: /^Network$/ }) })
  await row.waitFor({ timeout: 15000 })
  check((await row.locator('td.val').innerText()).trim() === 'pod', 'Network reads pod before')
  await row.locator('button', { hasText: 'Change' }).click()
  await row.locator('input').fill('stormbr1')
  await row.locator('button', { hasText: 'Save' }).click()
  await page.locator('p.saved').waitFor({ timeout: 10000 })
  const saved = await page.locator('p.saved').innerText()
  console.log(`    | ${saved}`)
  check(saved.includes('storm.io/bridge: stormbr1') && saved.includes('spec.networks'), 'the page says what was written')
  const after = (await row.locator('td.val').innerText()).trim()
  check(after === 'stormbr1', 'the field shows the edit at once', after)

  // The Overview's Network card: what was asked.
  await page.locator('nav.tabs button', { hasText: 'Overview' }).click()
  await page.getByText('host bridge stormbr1').first().waitFor({ timeout: 10000 }).catch(() => {})
  check((await page.getByText('host bridge stormbr1').count()) > 0, 'the Network card shows host bridge stormbr1')

  // The running machine: the pending notice.
  await page.goto(`${BASE}/#/vm/default/web`)
  await page.locator('nav.tabs button', { hasText: 'Settings' }).click()
  await page.getByText('Waiting for a restart.').waitFor({ timeout: 10000 }).catch(() => {})
  const pend = await page.locator('p.pending').first().innerText().catch(() => '')
  console.log(`    | ${pend}`)
  check(pend.includes('Network'), 'web: waiting for a restart, Network named')

  check(!errors.length, 'no page errors', errors.join('; '))
  await browser.close()
  process.exit(failed ? 1 : 0)
})().catch((e) => {
  console.log(`  FAIL ${e.message.split('\n')[0]}`)
  process.exit(1)
})
