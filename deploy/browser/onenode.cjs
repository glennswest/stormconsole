// A single node's console among other clusters' machines (#161): every page
// names this machine and no other; the Cluster page's form offers free nodes
// and nothing else, with a peer's network from its release flavour.
const { walk } = require('./lib.cjs')
const HOST = process.env.NODE_NAME
const OTHERS = /storm-003101|storm-003102|pvetest1|taken-1|gone-1|laptop-1|quiet-1/

walk('one node', async (t) => {
  const page = await t.page()
  const none = async (where) => {
    const text = await t.text(page)
    t.check(!OTHERS.test(text), `${where}: no other machine named`, (text.match(OTHERS) || [''])[0])
    return text
  }

  // Overview's plugin cards and counts come with the feed.
  t.check(await t.open(page, '#/', /Overview|Components/), 'the overview opens')
  await t.pause(1500)
  await none('overview')
  const nav = await page.locator('nav, aside').first().innerText().catch(() => '')
  t.check(!/flowsdn|pod network/i.test(nav), 'no pod network in the navigator', nav.slice(0, 200))
  await t.shot(page, 'onenode-overview')

  t.check(await t.open(page, '#/nodes', new RegExp(HOST)), 'Nodes names this node')
  const nodes = await none('Nodes')
  t.check(/1 node\b/.test(nodes), 'Nodes counts one', nodes.slice(0, 300))
  t.check(/storage 8\.9 TB free of 8\.9 TB, 206 volumes/.test(nodes), "with the storage engine's numbers on its row", nodes.slice(0, 400))
  t.check(!/stormstorage/.test(nodes), 'and no second row for the engine')
  await t.shot(page, 'onenode-nodes')

  t.check(await t.open(page, '#/drives', /5 drives\s+1 node/), 'Drives: 5 drives on 1 node')
  await none('Drives')
  await t.shot(page, 'onenode-drives')

  t.check(await t.open(page, '#/logs', /mine:/), 'Logs shows this node\'s lines')
  await t.pause(2500)
  const logs = await none('Logs')
  t.check(!/theirs:/.test(logs), "and none of another machine's", logs.slice(0, 300))
  await t.shot(page, 'onenode-logs')

  t.check(await t.open(page, '#/cluster', /single node/), 'Cluster shows this node as its only member')
  await t.pause(1500)
  let cluster = await none('Cluster')
  t.check(!/free-1|discovered/i.test(cluster), 'and names no free node outside the form', cluster.slice(0, 400))
  t.check(new RegExp(`${HOST}\\s+this node`, 'i').test(cluster), 'its row says so', cluster.slice(0, 400))
  await t.shot(page, 'onenode-cluster')

  await page.getByRole('button', { name: 'Form a cluster…' }).click()
  t.check(await t.until(async () => (await page.locator('input[aria-label="free-1 as master"]').count()) === 1), 'the form offers the free node')
  cluster = await none('the form')
  const row = await page.locator('table.pick tr', { hasText: 'free-1' }).innerText()
  t.check(/12\.61-flowsdn · flowsdn/.test(row), "the free node's network is its release's flavour, not what it announced", row)
  await t.shot(page, 'onenode-form')
})
