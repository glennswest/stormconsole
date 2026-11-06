// The datastore in a browser (#64), over deploy/verify-fastetcd.sh: real fastetcd.
const { walk } = require('./lib.cjs')

walk('datastore', async (t) => {
  const page = await t.page('root')
  await page.setViewportSize({ width: 1440, height: 1000 })
  t.check(await t.open(page, '#/etcd/keys', /registry/i), 'the keyspace opens')
  const body = () => page.locator('body').innerText()
  t.check(await t.until(async () => /pods/.test(await body())), 'grouped: pods', (await body()).slice(0, 160))
  await page.getByRole('link', { name: /pods/ }).first().click()
  t.check(await t.until(async () => /default/.test(await body())), 'and down into a project')
  t.check(await t.open(page, '#/etcd/keys?key=/registry/namespaces/default', /Namespace/), 'a value, decoded')
  await t.shot(page, 'datastore-value')
  const [dl] = await Promise.all([page.waitForEvent('download'), (async () => {
    await page.goto(`${process.env.CONSOLE}/#/etcd/keys`)
    await page.locator('a.dl').click()
  })()])
  t.check(/\.fbak$/.test(dl.suggestedFilename()), 'the snapshot downloads, named for what it is', dl.suggestedFilename())
  t.check(await t.open(page, '#/plugin/etcd', /fastetcd|datastore/i), "the plugin's page")
  // A row's numbers are on its opened line.
  await page.locator('tr', { hasText: 'datastore' }).first().getByRole('button', { name: 'Expand' }).click()
  t.check(await t.until(async () => /puts\/s/.test(await body())), 'the store: its rates', (await body()).slice(0, 400))
  t.check(/ranges\/s/.test(await body()) && /raft term/.test(await body()), 'and its raft numbers')
  await t.shot(page, 'datastore-store')
  t.check(await t.open(page, '#/grid?id=etcd:store&rel=members', /follower/), 'the members, and their roles', (await body()).slice(0, 400))
  t.check(/leader/.test(await body()), 'one of them the leader')
  await t.shot(page, 'datastore-card')
})
