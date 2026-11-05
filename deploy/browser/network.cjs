// Network Policies and Network Endpoints in a browser (#168), over
// deploy/verify-network-pages.sh. NET is the network this console's cluster runs: the
// pages must read the same, with its name only in the small "from:" line.
const { walk } = require('./lib.cjs')
const { NET, OTHER } = process.env

walk(`network pages (${NET})`, async (t) => {
  const page = await t.page('root')
  const product = /cilium|flowsdn/i
  const main = () => t.text(page)
  // Without the small line: nothing else may name a product.
  const words = async () => (await page.locator('main').innerText()).replace(/from: [a-z, ]+/g, '')

  t.check(await t.open(page, '#/network/policies', /allow-web/), 'Network Policies opens', (await main()).slice(0, 300))
  const nav = JSON.stringify(await page.evaluate(() => fetch('/api/v1/console/nav').then((r) => r.json())))
  t.check(nav.includes('Network Policies') && nav.includes('Network Endpoints'), 'the navigator has both')
  t.check(!/cilium/i.test(nav), 'and no product name in it', nav.match(/.{0,30}cilium.{0,30}/i)?.[0] || '')
  t.check(!product.test(await words()), 'no product name on the page', (await words()).match(/.{0,40}(cilium|flowsdn).{0,40}/i)?.[0] || '')
  t.check(new RegExp(`from: ${NET}`).test(await main()), `the small line says from: ${NET}`)

  const row = (name) => page.locator('table tbody tr', { hasText: name }).first()
  const allow = await row('allow-web').innerText()
  t.check(/shop/.test(allow) && /extended/.test(allow) && /app=web/.test(allow) && /2 rules/.test(allow) && /1 rule/.test(allow), 'a policy: project, selects, rules', allow)
  t.check(/all denied/.test(await row('deny-in').innerText()), 'a default deny is said', await row('deny-in').innerText())
  t.check(/cluster-wide/.test(await row('everyweb').innerText()), 'a cluster-wide one', await row('everyweb').innerText())
  await row('allow-web').getByRole('button').click()
  t.check(await t.until(async () => /shop\/web-1/.test(await row('allow-web').innerText())), 'which pods it applies to', await row('allow-web').innerText())
  await t.shot(page, `network-${NET}-policies`)

  // In its project: only the project's, and the cluster-wide one reaching in.
  t.check(await t.open(page, '#/network/policies?ns=lab', /Network Policies/), 'in a project')
  t.check(await t.until(async () => /everyweb/.test(await main()) === false && /lab-only/.test(await main())), 'the project with no web pods: its own, not the cluster-wide', (await main()).slice(0, 400))
  t.check(await t.open(page, '#/network/policies?ns=shop', /allow-web/), 'another project')
  t.check(await t.until(async () => /everyweb/.test(await main()) && !/lab-only/.test(await main())), 'its own, and the cluster-wide one reaching in', (await main()).slice(0, 400))

  // Endpoints.
  await page.goto(`${t.BASE}/#/`)
  t.check(await t.open(page, '#/network/endpoints', /10\.\d+\.\d+\.\d+/), 'Network Endpoints opens', (await main()).slice(0, 300))
  t.check(!product.test(await words()), 'no product name on the page', (await words()).match(/.{0,40}(cilium|flowsdn).{0,40}/i)?.[0] || '')
  t.check(new RegExp(`from: ${NET}`).test(await main()), `the small line says from: ${NET}`)
  const ep = await page.locator('table tbody tr').first().innerText()
  t.check(/not enforced/.test(await main()) && /enforced/.test(await main()), 'enforcement in and out', ep)
  t.check(/app=web/.test(await main()), 'an identity, by what it means')
  await t.shot(page, `network-${NET}-endpoints`)

  // A list that moved lands here, a project's link keeping its project.
  await page.goto(`${t.BASE}/#/k8s/cep?ns=shop`)
  t.check(await t.until(async () => page.url().includes('#/network/endpoints?ns=shop')), "a project's link keeps its project", page.url())
  await page.goto(`${t.BASE}/#/k8s/cnp`)
  t.check(await t.until(async () => page.url().includes('#/network/policies')), 'the old list goes to the new page', page.url())

  // A viewer in one project.
  const alice = await t.page('alice')
  t.check(await t.open(alice, '#/network/policies', /allow-web/), 'alice sees her project')
  const a = await t.text(alice)
  t.check(!/lab-only/.test(a), "and not another project's", a.slice(0, 300))

  // The other network is nowhere: not its plugin, nor its card.
  const feed = JSON.stringify(await page.evaluate(() => fetch('/api/v1/components').then((r) => r.json())))
  if (OTHER === 'flowsdn') t.check(!feed.includes('plugin:flowsdn') && !feed.includes('flowsdn:agent'), 'a cilium node carries nothing of flowsdn')
  if (OTHER === 'cilium') t.check(!feed.includes('k8s:cilium') && !/"Cilium/.test(feed), 'a flowsdn node carries nothing of cilium')
})
