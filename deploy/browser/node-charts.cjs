// The Node level's charts from cadvisor's optional groups (#386), over deploy/verify-node-charts.sh.
// HUGE: how many hugetlb series this machine's cadvisor serves.
const { walk } = require('./lib.cjs')
const HUGE = Number(process.env.HUGE || 0)

walk('node charts', async (t) => {
  const page = await t.page('root')
  t.check(await t.open(page, '#/metrics?level=node', /TCP connections, by state/), 'the Node level opens', (await t.text(page)).slice(0, 300))
  const drawn = (title) => page.locator('section.chart', { has: page.locator('h3', { hasText: title }) }).locator('svg')
  const charted = [
    'TCP connections, by state',
    'UDP sockets',
    'UDP bytes queued, receive and send',
    'UDP packets dropped, on open sockets',
    'CPU time spent waiting to run',
    'Scheduler latency: average wait per timeslice',
    'Waiting to run, by top-level cgroup',
  ]
  for (const title of charted) {
    t.check(await t.until(async () => (await drawn(title).count()) === 1, 30000), `${title}: drawn`)
  }
  const tcp = await page.locator('section.chart', { has: page.locator('h3', { hasText: 'TCP connections, by state' }) }).innerText()
  t.check(/listen/.test(tcp) && /established/.test(tcp), 'TCP lines named by state', tcp.slice(0, 300))
  const huge = page.locator('section.chart', { has: page.locator('h3', { hasText: 'Hugepages in use, by page size' }) })
  if (HUGE > 0) t.check(await t.until(async () => (await huge.locator('svg').count()) === 1), 'hugepages: drawn')
  else t.check(/cadvisor is up, and served none of this/.test(await huge.innerText()), 'hugepages: none here, said', await huge.innerText())
  const note = await page.locator('[data-testid=charts-from]').innerText()
  t.check(/no per-CPU chart/.test(note) && /cadvisor#40/.test(note), 'the level says why there is no per-CPU chart', note)
  t.check(!/per-CPU usage|CPU \d/.test(await page.locator('.charts').innerText()), 'and draws none')
  await t.shot(page, 'node-charts')

  // A level charted for another node stays on it from tab to tab. This console hears no fleet, which it says.
  page.expect5xx = (path) => path.startsWith('/api/plugins/metrics/charts')
  t.check(await t.open(page, '#/metrics?level=network&on=storm-z', /does not hear the fleet/), "another node's level: said whose", (await t.text(page)).slice(0, 300))
  t.check(/node storm-z/.test(await t.text(page)), 'the header names the node')
  await page.locator('nav.tabs button', { hasText: /^Node$/ }).click()
  t.check(await t.until(async () => (await page.evaluate(() => location.hash)) === '#/metrics?level=node&on=storm-z'), 'the Node tab keeps it', await page.evaluate(() => location.hash))
  await page.locator('nav.tabs button', { hasText: /^Targets$/ }).click()
  t.check(await t.until(async () => (await page.evaluate(() => location.hash)) === '#/metrics'), 'Targets are this node’s')
})
