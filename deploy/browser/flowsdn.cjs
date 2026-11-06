// The pod network page in a browser (#83), over deploy/verify-flowsdn.sh's
// stand-in flowsdn agent: every tab, the filters, an endpoint opened live,
// and a cilium node's console having no such page. (The
// navigator's item is checked through the nav feed by the rig: admin
// sections start shut.)
const { walk } = require('./lib.cjs')
const CILIUM = process.env.CILIUM

walk('flowsdn', async (t) => {
  const page = await t.page()
  t.check(await t.open(page, '#/flowsdn', /shop\/web-7d9/), 'the page lists the endpoints', (await t.text(page)).slice(0, 300))
  let text = await t.text(page)
  t.check(/flowsdn on node-a/.test(text), 'scoped to this node, as the agent names it')
  t.check(/loopback only, so each\s+node's console shows its own/.test(text), 'says it is this node only')
  const row = (s) => page.locator('table tbody tr', { hasText: s })
  t.check(/10\.5\.0\.7/.test(await row('shop/web-7d9').innerText()) && /31337/.test(await row('shop/web-7d9').innerText()), 'a row: pod first, its address and identity', await row('shop/web-7d9').innerText())
  t.check(/·\s*disconnecting/.test(await row('lab/batch-x').innerText()), 'not ready reads as · and its state', await row('lab/batch-x').innerText())
  await t.shot(page, 'flowsdn-endpoints')

  // The filters.
  await page.locator('select[aria-label="Project"]').selectOption('lab')
  t.check(await t.until(async () => (await page.locator('table tbody tr').count()) === 1), 'namespace lab: one row')
  await page.locator('select[aria-label="Project"]').selectOption('')
  await page.locator('input[aria-label="Search"]').fill('f00d::a05:0:0:7')
  t.check(await t.until(async () => (await page.locator('table tbody tr').count()) === 1), 'search by IPv6 address: one row')
  await page.locator('input[aria-label="Search"]').fill('')
  t.check((await page.locator('select[aria-label="Node"] option').allInnerTexts()).includes('node-a'), 'the node filter lists node-a')

  // One endpoint, live.
  await row('shop/web-7d9').getByRole('button').click()
  t.check(await t.until(async () => /Endpoint 1/i.test(await t.text(page))), 'the endpoint opens')
  text = await t.text(page)
  t.check(/connected ✓/.test(text), 'its link health, read live')
  t.check(/k8s:app=web/.test(text), 'its identity resolved to labels')
  t.check(/Deployment web/.test(text) && /02:00:00:00:05:01/.test(text), 'owner and MAC')
  await t.shot(page, 'flowsdn-endpoint')

  const tab = async (name, re, what) => {
    await page.locator('nav.tabs button', { hasText: name }).click()
    t.check(await t.until(async () => re.test(await t.text(page))), what, (await t.text(page)).slice(0, 400))
  }
  await tab('IPAM', /18446744073709551611/, 'IPAM: the IPv6 pool exact past 2^53')
  t.check(/249/.test(await t.text(page)) && /98% free/.test(await t.text(page)), 'IPAM: IPv4 249 available, 98% free')
  await tab('Health', /agent\.controllers/, 'Health: per module')
  t.check(/flowsdn does not have it yet/.test(await t.text(page)), 'Health: "not implemented" is said as not built, not broken')
  t.check(/direct node routes ✓/.test(await t.text(page)), 'Health: booleans as marks')
  await tab('Config', /datapath-mode veth/, 'Config: what the agent believes')
  await tab('Services', /default\/kubernetes/, 'Services: the frontends it programs')
  t.check(/10\.96\.44\.7:80\/TCP/.test(await t.text(page)) && /NodePort/.test(await t.text(page)), 'Services: frontend and type')
  await tab('Routes', /10\.6\.0\.0\/16/, 'Routes: the direct node routes')
  t.check(/skipped: gateway not on a directly connected network/.test(await t.text(page)), 'Routes: a skipped route says why')
  await tab('State', /agent\.restore/, 'State: the health table, as rows')
  await tab('Flows', /flowsdn#293/, 'Flows: says they wait on flowsdn#293')
  const v = page.locator('table.verdicts')
  t.check(await t.until(() => v.isVisible()), 'Flows: what the policy decided')
  const rows = (await v.locator('tbody tr').allInnerTexts()).map((r) => r.replace(/\s+/g, ' '))
  t.check(/denied/.test(rows[0]) && /this node/.test(rows[0]) && /23/.test(rows[0]), 'denied first, the most packets first: the node\'s own', rows[0])
  t.check(rows.some((r) => /lab\/batch-x egress denied TCP 5432 7/.test(r)), 'an endpoint by its pod', rows.join(' | '))
  t.check(rows.some((r) => /audit: would have been denied/.test(r)), 'audit said as what it means')
  t.check(rows.some((r) => /endpoint 99, no longer here/.test(r)), 'an endpoint no longer here')
  await t.shot(page, 'flowsdn-flows')

  // A cilium node: no page in the navigator, and its address lands on the network's page (#285).
  if (CILIUM) {
    const c = await page.context().newPage()
    await c.goto(`${CILIUM}/#/flowsdn`)
    t.check(await t.until(async () => c.url().includes('#/network/endpoints')), 'cilium node: the address lands on Network Endpoints', c.url())
    await c.screenshot({ path: `${process.env.SHOTS || '.'}/flowsdn-cilium.png` }).catch(() => {})
  }
})
