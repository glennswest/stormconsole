// The browser half of deploy/verify-vm-policy.sh (#51): a person opening an
// isolated project and a machine behind the hypervisor's NAT, and reading
// what the console says about policy. Any page error fails.
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

  // The project card: the badge, and the exception beside it.
  await page.goto(`${BASE}/#/k8s/ns/shop`)
  await page.locator('nav.tabs button', { hasText: 'Project' }).click()
  await page.getByText('Network isolation').waitFor({ timeout: 15000 })
  await page.getByText('which isolation does not reach').waitFor({ timeout: 10000 })
  const card = await page.locator('.card', { hasText: 'Network isolation' }).innerText()
  console.log(card.split('\n').map((l) => `    | ${l}`).join('\n'))
  check(card.includes('isolated'), 'the badge is shown')
  check(card.includes('Except 3 machines, which isolation does not reach'), 'the exception, beside the claim')
  for (const n of ['ghost', 'lan', 'nat']) check(card.includes(`${n} — `), `${n} listed`)
  check(!card.includes('pod — '), 'pod (an endpoint) not listed')
  check(card.includes('stormvm#16'), 'stormvm#16 named')
  check(!card.includes('Pods and machines in shop reach each other'), 'the old claim is gone')

  // The machine: its Network card says no policy applies.
  await page.locator('.card a', { hasText: 'nat' }).first().click()
  await page.getByText('No network policy applies to this machine.').waitFor({ timeout: 15000 })
  const net = await page.locator('.card', { hasText: 'Asked for' }).innerText()
  console.log(net.split('\n').map((l) => `    | ${l}`).join('\n'))
  check(net.includes('NAT inside the hypervisor, not a Cilium endpoint (stormvm#16)'), 'why, with stormvm#16')
  check(net.includes('shop is isolated, and this machine is outside that isolation'), 'the project is isolated and it is outside')
  check(net.includes('allow-web') && !net.includes('allow-db'), 'would be selected by allow-web, not allow-db')
  check(net.includes('They will apply once stormvm#16'), 'and when they will apply')

  // A machine on the pod network says nothing of the kind.
  await page.goto(`${BASE}/#/vm/shop/pod`)
  await page.getByText('Asked for').waitFor({ timeout: 15000 })
  check(!(await page.getByText('No network policy applies').count()), 'pod: no such sentence')

  // The list: the row's policy column.
  await page.goto(`${BASE}/#/vms?ns=shop`)
  await page.getByText('none applies (NAT)').first().waitFor({ timeout: 15000 }).catch(() => {})
  check((await page.getByText('none applies (NAT)').count()) > 0, 'the list shows "none applies (NAT)"')

  check(!errors.length, 'no page errors', errors.join('; '))
  await browser.close()
  process.exit(failed ? 1 : 0)
})().catch((e) => {
  console.log(`  FAIL ${e.message.split('\n')[0]}`)
  process.exit(1)
})
