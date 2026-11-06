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
  const text = await body()
  t.check(/puts\/s/.test(text) && /ranges\/s/.test(text), 'the rates on it', text.slice(0, 300))
  t.check(/leader/i.test(text), 'and its leader')
  await t.shot(page, 'datastore-card')
})
