// The Metrics page (#46), over deploy/verify-metrics.sh. PHASE: root, alice, gone.
const { walk } = require('./lib.cjs')
const PHASE = process.env.PHASE

walk(`metrics (${PHASE})`, async (t) => {
  if (PHASE === 'gone') {
    const page = await t.page('root')
    t.check(await t.open(page, '#/metrics', /ironprom is not started on this node/), 'no ironprom: said', (await t.text(page)).slice(0, 300))
    await t.shot(page, 'metrics-not-started')
    return
  }
  if (PHASE === 'alice') {
    const page = await t.page('alice')
    t.check(await t.open(page, '#/metrics?level=containers', /Container CPU/), 'her containers')
    t.check(await t.until(async () => (await page.locator('section.chart svg').count()) === 6), 'charted, CPU, memory and disk reads and writes apart')
    const html = await page.locator('main').innerHTML()
    t.check(/web/.test(html), 'her project', html.slice(0, 200))
    t.check(!/vault|secret/.test(html), 'and nothing of the other project')
    t.check(await t.open(page, '#/metrics?entity=pod&ns=secret&name=vault', /no project secret/), 'the other project is not there')
    return
  }
  const page = await t.page('root')
  t.check(await t.open(page, '#/metrics', /targets up/), 'Targets opens', (await t.text(page)).slice(0, 300))
  const nav = await page.evaluate(() => fetch('/api/v1/console/nav').then((r) => r.json()))
  t.check(JSON.stringify(nav.find((s) => s.label === 'Observe') || nav).includes('#/metrics'), 'Observe has Metrics', JSON.stringify(nav).slice(0, 200))
  const lb = page.locator('table.targets tr', { hasText: 'stormlb' })
  t.check(/down/.test(await lb.innerText()) && /refused|connect|error/i.test(await lb.innerText()), 'a down target and its last error', await lb.innerText())
  t.check((await page.locator('table.targets tbody tr').first().innerText()).includes('stormlb'), 'down first')
  t.check(/\bup\b/.test(await page.locator('table.targets tr', { hasText: 'stormdrive' }).innerText()), 'an up one')
  t.check(/Drives/.test(await page.locator('table.targets tr', { hasText: 'stormdrive' }).innerText()), 'with its level')
  await t.shot(page, 'metrics-targets')

  const charted = async (tab, want) => {
    await page.locator('nav.tabs button', { hasText: new RegExp(`^${tab}$`) }).click()
    const ok = await t.until(async () => (await page.locator('section.chart svg').count()) >= want, 60000)
    t.check(ok, `${tab}: at least ${want} chart(s)`, `${await page.locator('section.chart svg').count()}: ${(await t.text(page)).slice(0, 400)}`)
    await t.shot(page, `metrics-${tab.toLowerCase().replace(/\s+/g, '-')}`)
  }
  await charted('Drives', 5)
  t.check(/Drive read operations/.test(await t.text(page)) && /stormdrive#/.test(await t.text(page)), 'an empty disk chart says what it waits on', (await t.text(page)).slice(0, 400))
  t.check(/ZC100/.test(await page.locator('main').innerHTML()), 'a drive by its serial')
  await charted('Volumes', 2)
  t.check(/no series per volume/.test(await t.text(page)), 'says a volume has no series of its own')
  await charted('Containers', 6)
  t.check(/Container disk reads/.test(await t.text(page)) && /Container disk writes/.test(await t.text(page)), 'reads and writes apart')
  t.check(/vault/.test(await page.locator('main').innerHTML()), 'an administrator sees every project')
  await charted('Node', 4)
  t.check(await t.until(async () => /Node disk reads/.test(await t.text(page)) && /Node disk write operations/.test(await t.text(page))), "the node's disks", (await t.text(page)).slice(0, 300))
  await charted('API server', 2)
  await charted('Datastore', 1)
  await charted('Virtual machines', 3)
  await charted('Network', 1)

  await page.locator('select[aria-label=Window]').selectOption('21600')
  t.check(await t.until(async () => (await page.locator('section.chart svg').count()) === 1), 'a longer window')

  // From the objects.
  t.check(await t.open(page, '#/drives', /drive/), 'the drives page')
  t.check(await t.until(async () => (await page.locator('button.bay').count()) > 0), 'with a drive')
  await page.locator('button.bay').first().click()
  const drive = page.locator('section.picked a.metrics')
  t.check(await t.until(() => drive.isVisible()), 'a drive links to its series')
  await drive.click()
  t.check(await t.until(async () => /Drive ZC100/i.test(await t.text(page)) && (await page.locator('section.chart svg').count()) >= 2, 30000), 'its charts', (await t.text(page)).slice(0, 300))

  t.check(await t.open(page, '#/pod/shop/web', /Overview/), 'the pod page')
  await page.locator('nav.tabs button', { hasText: 'Stats' }).click()
  const disk = page.locator('[data-testid=disk-io]')
  t.check(await t.until(async () => (await disk.locator('section.chart svg').count()) === 4, 30000), "the pod's Stats tab charts its disk reads and writes apart", await disk.innerText().catch(() => ''))
  t.check(/Disk reads/.test(await disk.innerText()) && /Disk writes/.test(await disk.innerText()) && /Disk read operations/.test(await disk.innerText()) && /Disk write operations/.test(await disk.innerText()), 'bytes and operations, each way', await disk.innerText())
  await t.shot(page, 'pod-stats-disk')
  await page.getByRole('link', { name: /Its series in ironprom/ }).click()
  t.check(await t.until(async () => /Pod web in shop/i.test(await t.text(page)) && (await page.locator('section.chart svg').count()) === 8, 30000), 'a pod links to its series', (await t.text(page)).slice(0, 300))
  t.check(!/vault/.test(await page.locator('main').innerHTML()), 'its own and no other pod')

  t.check(await t.open(page, '#/vm/shop/vm1', /vm1/), 'the VM page')
  await page.locator('nav.tabs button', { hasText: 'Metrics' }).click()
  t.check(await t.until(async () => /Memory: allocated and in use/.test(await t.text(page)) && (await page.locator('section.chart svg').count()) === 11, 30000), 'a VM links to its series', (await t.text(page)).slice(0, 300))
  await t.shot(page, 'metrics-vm')
})
