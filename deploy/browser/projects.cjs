// Projects in a browser (#58, for #28), over deploy/verify-projects.sh's
// real rustkube with alice (admin of alice-work), bob (view on it) and root.
const { walk } = require('./lib.cjs')

walk('projects', async (t) => {
  const page = await t.page('alice')
  const main = () => t.text(page)
  t.check(await t.open(page, '#/projects', /alice-work/), 'Projects: alice-work is listed', (await main()).slice(0, 300))
  const picker = page.locator('#ns-pick')
  const opts = await picker.locator('option').allInnerTexts()
  t.check(opts.includes('alice-work') && !opts.includes('kube-system'), 'the masthead offers her projects, and no system namespace', opts.join(', '))

  // New project, from the page.
  await page.getByRole('button', { name: 'New project' }).first().click()
  await page.locator('input[aria-label="Name"]').fill('alice-two')
  await page.getByRole('button', { name: 'Create project' }).click()
  t.check(await t.until(async () => /alice-two/.test(await main()), 20000), 'New project: alice-two is created and listed', (await main()).slice(0, 300))
  t.check(await t.until(async () => (await picker.locator('option').allInnerTexts()).includes('alice-two')), 'and the masthead offers it')
  await t.shot(page, 'projects')

  // The project page: members and isolation, as its admin.
  t.check(await t.open(page, '#/k8s/ns/alice-work', /alice-work/), 'the alice-work page opens')
  await page.locator('nav.tabs button', { hasText: 'Project' }).click()
  const members = page.locator('section.card', { hasText: 'Members' })
  t.check(await t.until(async () => /bob/.test(await members.innerText())), 'Members: bob is there', await members.innerText().catch(() => ''))
  await page.locator('input[aria-label="Member"]').fill('carol')
  await page.locator('select[aria-label="Role"]').selectOption('edit')
  await page.getByRole('button', { name: 'Add member' }).click()
  t.check(await t.until(async () => /carol[\s\S]*edit/.test(await members.innerText())), 'Add member: carol is edit', await members.innerText())
  await members.locator('tr', { hasText: 'carol' }).getByRole('button', { name: 'Remove' }).click()
  t.check(await t.until(async () => !/carol/.test(await members.innerText())), 'Remove: carol is gone')

  const iso = page.locator('.card', { hasText: 'Network isolation' })
  await iso.getByRole('button', { name: 'Isolate this project' }).click()
  t.check(await t.until(async () => /isolated/i.test(await iso.innerText()) && /cluster DNS/.test(await iso.innerText())), 'Isolate: the badge, with DNS', await iso.innerText())
  await t.shot(page, 'project-isolated')
  await iso.getByRole('button', { name: 'Remove isolation' }).click()
  t.check(await t.until(async () => /Not isolated/.test(await iso.innerText())), 'Remove isolation', await iso.innerText())

  // bob is a viewer of alice-work: he reads it, and the apiserver refuses his change.
  const bob = await t.page('bob')
  t.check(await t.open(bob, '#/projects', /alice-work/), 'bob sees alice-work')
  await t.open(bob, '#/k8s/ns/alice-work', /alice-work/)
  await bob.locator('nav.tabs button', { hasText: 'Project' }).click()
  const biso = bob.locator('.card', { hasText: 'Network isolation' })
  await t.until(async () => (await biso.count()) > 0)
  const isoBtn = biso.getByRole('button', { name: 'Isolate this project' })
  if (await isoBtn.count()) {
    await isoBtn.click()
    t.check(await t.until(async () => /forbidden|not allowed|may not|403/i.test(await bob.locator('p.error').innerText())), "bob's isolate: the apiserver's refusal, in words", await bob.locator('p.error').innerText().catch(() => ''))
  } else {
    t.check(true, 'bob is offered no isolate')
  }

  // root sees the system namespaces apart.
  const root = await t.page('root')
  await t.open(root, '#/projects', /alice-work/)
  const groups = await root.locator('#ns-pick optgroup').evaluateAll((gs) => gs.map((g) => g.label + ':' + [...g.querySelectorAll('option')].map((o) => o.value).join(',')))
  t.check(groups.some((g) => g.startsWith('System:') && g.includes('kube-system')), 'root: system namespaces in their own group', groups.join(' | '))
})
