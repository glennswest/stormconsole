// The grid for every custom resource (#348), over verify-crd-grid.sh's real
// apiserver: the CRD list, raidsets with stormraid's columns and the phase
// bar, live; drives with health and link; an object's page; a kind with no
// columns and the project filter; bob refused.
const { walk } = require('./lib.cjs')

walk('crds', async (t) => {
  const page = await t.page('root')
  const heads = async () => (await page.locator('main table thead th').allInnerTexts()).map((s) => s.trim().toUpperCase())

  // The sidebar's Custom resources is the list; the old address lands there.
  await page.goto(`${t.BASE}/#/k8s/crd`)
  t.check(await t.until(async () => /#\/crds$/.test(page.url())), 'the old #/k8s/crd lands on #/crds', page.url())
  t.check(await t.open(page, '#/crds', /raidsets\.storage\.storm\.io/), 'the CRD list', (await t.text(page)).slice(0, 300))
  const rs = await page.locator('main tr', { hasText: 'raidsets.storage.storm.io' }).innerText()
  t.check(/RaidSet/.test(rs) && /storage\.storm\.io/.test(rs) && /v1alpha1/.test(rs) && /Cluster/.test(rs) && /\b1\b/.test(rs) && /\b5\b/.test(rs), 'its row: kind, group, version, scope, 1 instance, 5 columns', rs)
  await t.shot(page, 'crds-list')
  await page.locator('input[type=search]').fill('rs')
  t.check(await t.until(async () => (await page.locator('main tbody tr').count()) === 1), 'searching its short name finds it alone')

  // The instances grid.
  await page.locator('main tbody tr', { hasText: 'raidsets' }).click()
  t.check(await t.until(async () => /#\/cr\/raidsets\.storage\.storm\.io$/.test(page.url())), 'a row opens its instances', page.url())
  t.check(await t.until(async () => /md0/.test(await t.text(page))), 'md0 is listed')
  t.check(JSON.stringify(await heads()) === JSON.stringify(['NAME', 'NODE', 'PARITY', 'PHASE', 'DEVICE', 'AGE']), 'NAME, the printer columns, AGE', JSON.stringify(await heads()))
  const row = () => page.locator('main tbody tr', { hasText: 'md0' }).innerText()
  t.check(/forge1/.test(await row()) && /▰▰▰▱ Rebuilding/.test(await row()) && /\/dev\/md0/.test(await row()), 'the row, its phase as a bar', await row())
  t.check(/● live/.test(await t.text(page)), 'it says it is live')
  await page.getByLabel(/Wide/).check()
  t.check(await t.until(async () => (await heads()).includes('MESSAGE')), 'Wide adds Message (priority 1)')
  t.check(/slot 1 at 41%/.test(await row()), 'with its value')
  await t.shot(page, 'crds-raidsets')

  // Live: a change made elsewhere arrives without a reload.
  const ctx = page.context().request
  const obj = await (await ctx.get(`${t.BASE}/api/plugins/k8s/cr/raidsets.storage.storm.io/object/_/md0`)).json()
  const ready = obj.yaml.replace(/phase: Rebuilding/, 'phase: Ready')
  const put = await ctx.put(`${t.BASE}/api/plugins/k8s/cr/raidsets.storage.storm.io/object/_/md0`, { data: ready, headers: { 'content-type': 'application/yaml' } })
  t.check(put.ok(), 'md0 set Ready through the console', `${put.status()}`)
  t.check(await t.until(async () => /▰▰▰▰ Ready/.test(await row()), 10000), 'the grid shows Ready without a reload', await row())

  // The object.
  await page.locator('main tbody tr', { hasText: 'md0' }).click()
  t.check(await t.until(async () => /#\/cr\/raidsets\.storage\.storm\.io\/_\/md0$/.test(page.url())), 'a row opens the object', page.url())
  t.check(await t.until(async () => /Conditions/i.test(await t.text(page))), 'its page')
  const text = await t.text(page)
  t.check(/NODE\s+forge1/i.test(text) && /PARITY\s+1/i.test(text), 'the printer values on top', text.slice(0, 400))
  t.check(/Degraded\s+True\s+MemberMissing\s+slot 1 is rebuilding/.test(text), 'the conditions as a table')
  t.check((await page.locator('dt.hl', { hasText: 'node' }).count()) === 1, 'a field a printer column shows is marked')
  t.check((await page.getByRole('button', { name: 'Delete' }).isEnabled()) && (await page.getByRole('button', { name: 'Edit' }).isEnabled()), 'Edit and Delete offered to root')
  await t.shot(page, 'crds-object')
  await page.locator('nav.tabs button', { hasText: 'Events' }).click()
  t.check(await t.until(async () => /slot 1 rebuilding from 5000c5002/.test(await t.text(page))), 'its events')
  await page.locator('nav.tabs button', { hasText: 'YAML' }).click()
  t.check(await t.until(async () => /kind: RaidSet/.test(await t.text(page))), 'its YAML')
  t.check((await page.getByRole('button', { name: /Copy the YAML/ }).count()) === 1, 'with copy')

  // A drive: health and link, and its owner as a link.
  t.check(await t.open(page, '#/cr/drives.storage.storm.io', /wwn-5000c5002/), 'the drives grid')
  const d2 = await page.locator('main tbody tr', { hasText: 'wwn-5000c5002' }).innerText()
  t.check(/● Failed/.test(d2) && /● degraded/.test(d2), 'health and link as dots', d2)
  const failed = page.locator('main tbody tr', { hasText: 'wwn-5000c5002' }).locator('.mv.error')
  t.check((await failed.count()) === 1, 'Failed in the error colour')
  await t.shot(page, 'crds-drives')
  await page.locator('main tbody tr', { hasText: 'wwn-5000c5002' }).click()
  t.check(await t.until(async () => (await page.getByRole('link', { name: 'RaidSet md0' }).count()) === 1), 'the owner is a link')
  await page.getByRole('link', { name: 'RaidSet md0' }).click()
  t.check(await t.until(async () => /#\/cr\/raidsets\.storage\.storm\.io\/_\/md0$/.test(page.url())), 'to its RaidSet')

  // No columns, and the project filter.
  t.check(await t.open(page, '#/cr/widgets.example.io?ns=', /w3/), 'widgets, every project')
  t.check(/declares no printer columns/.test(await t.text(page)), 'it says it has no printer columns')
  t.check(JSON.stringify(await heads()) === JSON.stringify(['NAME', 'NAMESPACE', 'PHASE', 'REPLICAS', 'MESSAGE', 'AGE']), 'NAME, NAMESPACE, the status fields, AGE', JSON.stringify(await heads()))
  await page.locator('main select[aria-label="Project"]').selectOption('team-a')
  t.check(await t.until(async () => /ns=team-a/.test(page.url()) && /w1/.test(await t.text(page)) && !/w3/.test(await t.text(page))), 'team-a: its widgets only', page.url())
  t.check(!(await heads()).includes('NAMESPACE'), 'and no project column')
  await t.shot(page, 'crds-widgets')

  // Bob may not list RaidSets, and is told so.
  const bob = await t.page('bob')
  t.check(await t.open(bob, '#/cr/raidsets.storage.storm.io', /may not list raidsets/), 'bob is told he may not list them', (await t.text(bob)).slice(0, 300))
  t.check(await t.open(bob, '#/crds', /not yours to list/), 'and the list says so instead of a count')
})
