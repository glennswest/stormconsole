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
  await page.locator('nav.tabs button', { hasText: 'Access' }).click()
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
  await bob.locator('nav.tabs button', { hasText: 'Access' }).click()
  const biso = bob.locator('.card', { hasText: 'Network isolation' })
  await t.until(async () => (await biso.count()) > 0)
  const isoBtn = biso.getByRole('button', { name: 'Isolate this project' })
  if (await isoBtn.count()) {
    await isoBtn.click()
    t.check(await t.until(async () => /forbidden|not allowed|may not|403/i.test(await bob.locator('p.error').innerText())), "bob's isolate: the apiserver's refusal, in words", await bob.locator('p.error').innerText().catch(() => ''))
  } else {
    t.check(true, 'bob is offered no isolate')
  }

  // #162 --------------------------------------------------------------------
  // alice's project page holds everything in it, and opening it is being in it.
  await page.goto(`${t.BASE}/#/projects`) // the same hash again would stay on its tab
  t.check(await t.open(page, '#/k8s/ns/alice-work', /Inventory/), 'the project page opens on its inventory')
  const inv = page.locator('.card.inventory')
  const invText = () => inv.innerText()
  for (const w of ['Virtual machines', 'Config maps', 'Load balancers', 'Network attachments', 'Services', 'Pods', 'PersistentVolumeClaims']) {
    t.check(await t.until(async () => (await invText()).includes(w)), `the inventory counts ${w}`, await invText())
  }
  t.check(await picker.inputValue() === 'alice-work', 'opening a project selects it in the masthead', await picker.inputValue())
  const sec = page.locator('.card', { hasText: 'Secrets' })
  t.check(await t.until(async () => /db-pass[\s\S]*password/.test(await sec.innerText())), 'Secrets: the name and its keys', await sec.innerText())
  t.check(!/aHVudGVy|hunter/.test(await sec.innerText()), 'and no value')
  await t.shot(page, 'project-page')
  await inv.locator('a', { hasText: 'Config maps' }).click()
  t.check(await t.until(async () => /app-conf/.test(await main())), 'Config maps links to its rows', (await main()).slice(0, 200))
  const crumbs = await page.locator('.sc-crumbs').first().innerText().catch(() => '')
  t.check(/Projects[\s\S]*alice-work[\s\S]*Config maps/i.test(crumbs), 'the list is inside its project: Projects › alice-work › Config maps', crumbs)
  t.check(await t.open(page, '#/k8s/nad?ns=alice-work', /lan/), 'a network attachment has a row')
  if (process.env.ROUTE_SERVED === '1') t.check(await t.open(page, '#/k8s/route?ns=alice-work', /www\.example\.lo/), 'a route has a row')
  // The masthead moves the page, and a create lands in the project you are in.
  t.check(await t.open(page, '#/k8s/ns/alice-work', /Inventory/), 'back on alice-work')
  await picker.selectOption('alice-two')
  t.check(await t.until(async () => page.url().includes('#/k8s/ns/alice-two')), 'choosing another project in the masthead opens it', page.url())
  await t.until(async () => /Inventory/i.test(await main()))
  await page.locator('main button.create, .create').last().click()
  await page.getByRole('menuitem').filter({ hasText: /Import YAML/ }).click()
  const dest = page.locator('[role=dialog] select[aria-label=Project]')
  t.check(await t.until(async () => (await dest.inputValue()) === 'alice-two'), 'Create from a project targets it', await dest.inputValue().catch(() => ''))
  await page.keyboard.press('Escape')

  // bob reads only what his role lets him: whatever the answer, no value.
  const bobSecrets = await bob.evaluate(() => fetch('/api/plugins/k8s/projects/alice-work/secrets').then((r) => r.text()))
  t.check(!/aHVudGVy/.test(bobSecrets) && (/refused|db-pass/.test(bobSecrets)), "bob's secrets answer has no value in it", bobSecrets)

  // root sees the system namespaces apart.
  const root = await t.page('root')
  await t.open(root, '#/projects', /alice-work/)
  const groups = await root.locator('#ns-pick optgroup').evaluateAll((gs) => gs.map((g) => g.label + ':' + [...g.querySelectorAll('option')].map((o) => o.value).join(',')))
  t.check(groups.some((g) => g.startsWith('System:') && g.includes('kube-system')), 'root: system projects in their own group', groups.join(' | '))

  // One page: every project, the platform's marked, filterable.
  const row = (n) => root.locator('table.projects tr', { hasText: n }).first()
  const rootText = () => t.text(root)
  t.check(await t.until(async () => /storm-assets/.test(await rootText())), 'Projects lists storm-assets', (await rootText()).slice(0, 400))
  for (const n of ['storm-assets', 'cilium-secrets', 'kube-system', 'default', 'cilium']) {
    t.check(/system/i.test(await row(n).innerText()), `${n} is marked system`, await row(n).innerText())
  }
  t.check(!/system/i.test(await row('alice-work').innerText()), 'alice-work is not', await row('alice-work').innerText())
  t.check(/\d+ pods?/.test(await row('alice-work').innerText()), 'a row says what the project holds', await row('alice-work').innerText())
  await t.shot(root, 'projects-all')
  const which = root.locator('select[aria-label="Which projects"]')
  await which.selectOption('system')
  t.check(await t.until(async () => !/alice-work/.test(await root.locator('table.projects').innerText())), 'filter System: alice-work is gone')
  t.check(/storm-assets/.test(await root.locator('table.projects').innerText()), 'and storm-assets stays')
  await which.selectOption('mine')
  t.check(await t.until(async () => !/storm-assets/.test(await root.locator('table.projects').innerText())), 'filter Not system: storm-assets is gone')
  t.check(/alice-work/.test(await root.locator('table.projects').innerText()), 'and alice-work stays')

  // No second page for it.
  await root.goto(`${t.BASE}/#/k8s/ns`)
  t.check(await t.until(async () => root.url().endsWith('#/projects')), 'the old list redirects to Projects', root.url())
  const navItems = await root.evaluate(() => fetch('/api/v1/console/nav').then((r) => r.json()))
  const labels = JSON.stringify(navItems)
  t.check(!/Namespaces|#\/k8s\/ns/.test(labels), 'the navigator has no Namespaces', labels.slice(0, 300))
  t.check(/Routes/.test(labels) && /Network attachments/.test(labels) && /Config maps/.test(labels), 'and has what a project holds')

  // Lists show their project: a column, and a header per project when several.
  // Config maps: the rig has them in alice-work and kube-system (root's note).
  t.check(await t.open(root, '#/k8s/cm?ns=', /root-note/), 'all projects: config maps')
  t.check(/^project$/i.test((await root.locator('th.place').first().innerText()).replace(/[^a-z]/gi, '')), 'the column is called Project', await root.locator('th.place').first().innerText())
  const heads = await root.locator('tr.group').allInnerTexts()
  t.check(heads.some((h) => /alice-work/i.test(h)) && heads.some((h) => /kube-system[\s\S]*system/i.test(h)), 'grouped by project, the system one marked', heads.join(' | '))
  await t.shot(root, 'config-maps-by-project')

  // One word. YAML is the API's, and the Projects page says once what a
  // project is; everything else a person reads says project.
  const words = () =>
    root.evaluate(() => {
      const out = [document.body.innerText]
      for (const el of document.querySelectorAll('[title],[aria-label],[placeholder]')) {
        for (const a of ['title', 'aria-label', 'placeholder']) if (el.getAttribute(a)) out.push(el.getAttribute(a))
      }
      for (const o of document.querySelectorAll('option')) out.push(o.textContent)
      return out.join('\n')
    })
  const said = (text) => text.split('\n').filter((l) => /namespace/i.test(l) && !/^A project is a namespace/.test(l.trim()))
  const pages = ['#/', '#/projects', '#/projects?new=1', '#/k8s/pod', '#/k8s/svc', '#/k8s/cm', '#/k8s/pvc', '#/k8s/netpol', '#/k8s/nad', '#/k8s/route', '#/k8s/events', '#/vms', '#/k8s/ns/alice-work', '#/k8s/ns/kube-system', '#/account/keys']
  for (const p of pages) {
    await root.goto(`${t.BASE}/${p}`)
    await t.pause(1500)
    const bad = said(await words())
    t.check(bad.length === 0, `${p}: says project`, bad.slice(0, 3).join(' | '))
  }
  await t.open(root, '#/k8s/ns/alice-work', /Inventory/)
  for (const tab of ['Access', 'Resources', 'Events']) {
    await root.locator('nav.tabs button', { hasText: tab }).click()
    await t.pause(1000)
    const bad = said(await words())
    t.check(bad.length === 0, `the project's ${tab} tab: says project`, bad.slice(0, 3).join(' | '))
  }
  // A system project takes no Create from its page.
  await t.open(root, '#/k8s/ns/kube-system', /Inventory/)
  t.check(/system project/i.test(await t.text(root)), 'a system project says so on its page')
})
