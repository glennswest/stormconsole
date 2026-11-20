// Another node's charts (#369), over deploy/verify-remote-charts.sh: storm-b's objects drawn from storm-b's
// ironprom through this console, and the refusals said on the page.
const { walk } = require('./lib.cjs')

walk('remote charts', async (t) => {
  const page = await t.page('root')
  const drawn = (n) => t.until(async () => (await page.locator('section.chart svg').count()) >= n, 30000)
  const from = () => page.getByTestId('charts-from').innerText().catch(() => '')

  t.check(await t.open(page, '#/vm/shop/vm-b', /vm-b/), 'the VM page of a machine on storm-b')
  await page.locator('nav.tabs button', { hasText: 'Metrics' }).click()
  t.check(await drawn(1), 'its Metrics tab draws its CPU', (await t.text(page)).slice(0, 400))
  t.check(/From storm-b.s ironprom/.test(await from()), 'and says the series are storm-b’s', await from())
  await t.shot(page, 'remote-vm-metrics')

  t.check(await t.open(page, '#/metrics?entity=drive&serial=B200&on=storm-b', /Drive B200/), 'storm-b’s drive on the Metrics page')
  t.check(await drawn(1), 'charted', (await t.text(page)).slice(0, 400))
  t.check(/node storm-b/i.test(await page.locator('main').innerText()), 'the page’s scope is storm-b')
  t.check(/From storm-b.s ironprom/.test(await from()), 'from storm-b’s ironprom', await from())
  await t.shot(page, 'remote-drive')

  t.check(await t.open(page, '#/metrics?entity=port&node=storm-b&ifname=ens1f0', /Port ens1f0 on storm-b/), 'storm-b’s port')
  t.check(await drawn(1), 'charted from its own node', (await t.text(page)).slice(0, 400))

  t.check(await t.open(page, `#/metrics?entity=drive&serial=A100`, /Drive A100/), 'this node’s drive')
  t.check(await drawn(1), 'charted')
  t.check(new RegExp(`From ${process.env.HOST}.s ironprom`).test(await from()), 'from this node’s ironprom', await from())

  page.upstreamDown = true
  t.check(await t.open(page, '#/metrics?entity=vm&ns=shop&name=x&on=storm-c', /storm-c did not answer/), 'a node that does not answer is said', (await t.text(page)).slice(0, 300))
  t.check((await page.locator('section.chart').count()) === 0, 'not an empty chart')
  await t.shot(page, 'remote-unreachable')
  t.check(await t.open(page, '#/metrics?entity=vm&ns=shop&name=x&on=nowhere', /no node nowhere/), 'a node not in this cluster is said')

  const alice = await t.page('alice')
  t.check(await t.open(alice, '#/metrics?entity=vm&ns=shop&name=vm-b&on=storm-b', /Virtual machine vm-b in shop/), 'alice: her machine on storm-b')
  t.check(await t.until(async () => (await alice.locator('section.chart svg').count()) >= 1, 30000), 'charted for her')
  t.check(await t.open(alice, '#/metrics?entity=vm&ns=secret&name=vm-s&on=storm-b', /no project secret/), 'alice: another project’s machine on storm-b is not there')
})
