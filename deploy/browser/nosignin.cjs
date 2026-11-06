// No sign-in is root (#165), over verify-no-signin.sh's real apiserver.
const { walk } = require('./lib.cjs')

walk('no sign-in', async (t) => {
  const p = await t.page('')
  await t.open(p, '#/k8s/ns/shop', /Inventory/)
  await p.locator('nav.tabs button', { hasText: 'Access' }).click()
  await t.until(async () => (await p.locator('section.card', { hasText: 'Members' }).count()) > 0)
  for (const b of ['Add member', 'Isolate this project', 'Delete shop']) {
    t.check((await p.getByRole('button', { name: b }).count()) === 1, `the project page offers ${b}`)
  }
  await t.shot(p, 'nosignin-project')
  t.check(await t.open(p, '#/k8s/pod', /web-0/), 'the pods list')
  await p.locator('tr', { hasText: 'web-0' }).first().getByRole('button', { name: 'Expand' }).click()
  t.check((await p.locator('.all-acts').getByRole('button', { name: 'Delete' }).count()) === 1, 'a pod offers Delete')
  await p.goto(`${t.BASE}/#/pod/shop/web-0`)
  await p.locator('nav.tabs button', { hasText: 'Terminal' }).click()
  t.check(await t.until(async () => (await p.getByRole('button', { name: /^Connect/ }).count()) === 1), 'the Terminal offers Connect')
  t.check(!/no kubernetes identity|signed in/.test(await t.text(p)), 'with no word about signing in')
  await p.locator('nav.tabs button', { hasText: 'YAML' }).click()
  t.check(await t.until(async () => (await p.getByRole('button', { name: 'Edit' }).count()) === 1), 'YAML offers Edit')
  t.check(await t.open(p, '#/node/n1', /Workloads on this node/), "a node's page")
  const card = p.locator('section[aria-label="Drain"]')
  t.check(await t.until(async () => (await card.getByRole('button', { name: /Drain|Cordon/ }).count()) > 0), 'offers Cordon or Drain', await card.innerText().catch(() => ''))
  t.check(!/no kubernetes identity/.test(await card.innerText()), 'with no refusal')
  await t.shot(p, 'nosignin-node')
})
