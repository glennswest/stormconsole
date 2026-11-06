// What a viewer is offered (#45), over verify-access-reviews.sh's real apiserver: alice is admin of shop, bob has view.
const { walk } = require('./lib.cjs')

walk('access', async (t) => {
  const accessTab = async (page) => {
    await t.open(page, '#/k8s/ns/shop', /Inventory/)
    await page.locator('nav.tabs button', { hasText: 'Access' }).click()
    await t.until(async () => (await page.locator('section.card', { hasText: 'Members' }).count()) > 0)
  }
  const named = (page, name) => page.getByRole('button', { name }).count()

  const alice = await t.page('alice')
  await accessTab(alice)
  t.check((await named(alice, 'Add member')) === 1, 'alice, admin of shop: Add member')
  t.check((await named(alice, 'Isolate this project')) === 1, 'Isolate')
  t.check((await named(alice, 'Remove')) > 0, 'Remove on the members')
  t.check((await named(alice, 'Delete shop')) === 1, 'Delete shop')
  await t.shot(alice, 'access-admin')
  t.check(await t.open(alice, '#/k8s/pod', /web-0/), 'the pods list')
  await alice.locator('tr', { hasText: 'web-0' }).first().getByRole('button', { name: 'Expand' }).click()
  t.check((await alice.locator('.all-acts').getByRole('button', { name: 'Delete' }).count()) === 1, 'an opened pod row offers her Delete')
  await alice.goto(`${t.BASE}/#/pod/shop/web-0?tab=YAML`)
  await alice.locator('nav.tabs button', { hasText: 'YAML' }).click().catch(() => {})
  t.check(await t.until(async () => (await named(alice, 'Edit')) === 1), "a pod's YAML offers her Edit")

  const bob = await t.page('bob')
  await accessTab(bob)
  const members = bob.locator('section.card', { hasText: 'Members' })
  // `view` holds no rolebindings: the apiserver's answer is what the page says.
  t.check(await t.until(async () => /may not change who is in/.test(await members.innerText())), 'bob, view on shop: told he may not change who is in it', await members.innerText())
  for (const b of ['Add member', 'Remove', 'Isolate this project', 'Delete shop']) {
    t.check((await named(bob, b)) === 0, `no ${b}`)
  }
  await t.shot(bob, 'access-view')
  await bob.locator('nav.tabs button', { hasText: 'YAML' }).click()
  t.check(await t.until(async () => /kind: Namespace/.test(await t.text(bob))), "the project's YAML reads", (await t.text(bob)).slice(0, 200))
  t.check((await named(bob, 'Edit')) === 0, 'with no Edit')
  t.check(await t.open(bob, '#/k8s/pod', /web-0/), 'the pods list')
  await bob.locator('tr', { hasText: 'web-0' }).first().getByRole('button', { name: 'Expand' }).click()
  const acts = bob.locator('.all-acts')
  t.check(await t.until(async () => /Logs/.test(await acts.innerText())), 'an opened pod row offers him Logs', await acts.innerText().catch(() => ''))
  t.check((await acts.getByRole('button', { name: 'Delete' }).count()) === 0, 'and no Delete')
  await t.shot(bob, 'access-view-row')
  await bob.goto(`${t.BASE}/#/pod/shop/web-0?tab=YAML`)
  await bob.locator('nav.tabs button', { hasText: 'YAML' }).click().catch(() => {})
  t.check(await t.until(async () => /kind: Pod/.test(await t.text(bob))), "a pod's YAML reads", (await t.text(bob)).slice(0, 200))
  t.check((await named(bob, 'Edit')) === 0, 'with no Edit')
})
