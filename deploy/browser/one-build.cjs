// One console for every build (#285), over deploy/verify-network-pages.sh: every page a person would
// open on a node running NET, with OTHER's CRDs served and its agent answering, reads nothing of OTHER.
const { walk } = require('./lib.cjs')
const { NET, OTHER } = process.env

walk(`one build (${NET})`, async (t) => {
  const page = await t.page('root')
  const other = new RegExp(OTHER, 'i')
  const whole = async () => page.locator('body').innerText()
  // Shut sections would hide what the navigator offers.
  const nav = JSON.stringify(await page.evaluate(() => fetch('/api/v1/console/nav').then((r) => r.json())))
  t.check(!other.test(nav), `the navigator has nothing of ${OTHER}`, nav.match(new RegExp(`.{0,30}${OTHER}.{0,30}`, 'i'))?.[0] || '')

  const pages = [
    ['#/', /Overview/i],
    ['#/k8s/pod', /web-1/],
    ['#/pod/shop/web-1', /web-1/],
    ['#/network/policies', /allow-web/],
    ['#/network/endpoints', /web-1/],
    ['#/projects', /shop/],
    ['#/k8s/ns/shop', /shop/],
    ['#/metrics', /stormdrive/],
    ['#/k8s/svc', /Services/i],
  ]
  for (const [hash, wants] of pages) {
    t.check(await t.open(page, hash, wants), `${hash} opens`, (await t.text(page)).slice(0, 200))
    await t.pause(600)
    let text = await whole()
    // The small "from:" line names this network; the other's is what must not be here.
    t.check(!other.test(text), `${hash}: nothing of ${OTHER}`, text.match(new RegExp(`.{0,50}${OTHER}.{0,50}`, 'i'))?.[0] || '')
    t.check(!/stale-other|stale-node/.test(text), `${hash}: none of its objects`)
  }
  await t.shot(page, `one-build-${NET}-overview`)

  // A pod's Network tab: the network's own objects, and flows only where this network has them.
  t.check(await t.open(page, '#/pod/shop/web-1?tab=Network', /Pod network/), 'the pod page\'s Network tab', (await t.text(page)).slice(0, 200))
  await page.getByRole('button', { name: 'Network', exact: true }).click().catch(() => {})
  t.check(await t.until(async () => /Datapath/.test(await t.text(page))), 'its endpoint: datapath', (await t.text(page)).slice(0, 400))
  const net = await t.text(page)
  t.check(/app=web/.test(net), 'its identity, by what it means')
  t.check(/allow-web/.test(net), 'the policy that selects it')
  t.check(!/stale-other/.test(net), `not ${OTHER}'s`)
  const flows = await page.locator('[aria-label="Flows"]').count()
  t.check(flows === (NET === 'cilium' ? 1 : 0), NET === 'cilium' ? 'the Flows card' : 'no Flows card', String(flows))
  await t.shot(page, `one-build-${NET}-pod`)

  // A project says who enforces its isolation without naming anyone.
  t.check(await t.open(page, '#/k8s/ns/shop', /shop/), 'the project')
  await page.getByRole('button', { name: /isolation|network/i }).first().click().catch(() => {})
  await t.pause(500)
  t.check(!/cilium/i.test((await whole()).replace(/from: cilium/g, '')) || NET === 'cilium', 'the isolation sentence names no product on flowsdn')

  // The Create menu.
  const creators = JSON.stringify(await page.evaluate(() => fetch('/api/v1/console/creators').then((r) => r.json())))
  t.check(!other.test(creators), `Create offers nothing of ${OTHER}`)

  // The other's addresses land here.
  const lands = NET === 'cilium'
    ? [['#/flowsdn', '#/network/endpoints'], ['#/k8s/fsid', '#/k8s/cid'], ['#/k8s/fsnp', '#/network/policies']]
    : [['#/network/flows', '#/flowsdn'], ['#/k8s/cn', '#/flowsdn'], ['#/k8s/cid', '#/k8s/fsid'], ['#/k8s/cnp', '#/network/policies']]
  for (const [from, to] of lands) {
    await page.goto(`${t.BASE}/#/`)
    await t.pause(200)
    await page.goto(`${t.BASE}/${from}`)
    t.check(await t.until(async () => page.url().includes(to)), `${from} lands on ${to}`, page.url())
  }
  if (NET === 'flowsdn') {
    t.check(await t.open(page, '#/flowsdn', /Pod network/), 'its own page')
    await t.shot(page, 'one-build-flowsdn-page')
    t.check(await t.open(page, '#/k8s/fsid', /31337/), 'its identities')
  } else {
    t.check(await t.open(page, '#/network/flows', /Network Flows/), 'its Flows page')
    t.check(await t.open(page, '#/k8s/cid', /31337/), 'its identities')
  }
})
