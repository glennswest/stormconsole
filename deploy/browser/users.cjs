// Admin → Users (#266): admin signs in on the login page with the install's
// password, opens Users from the navigator, makes a storage-admin, resets a
// password and deletes a user by typing the name. A storage-admin is not
// offered the page and is refused its API. Groups (#334): a group made from the
// form with carol in it, shown on her row, deleted by typing its name.
const { walk } = require('./lib.cjs')
const ADMIN_PW = process.env.ADMIN_PW

walk('users', async (t) => {
  // The login page, as a person would.
  const page = await t.page()
  await page.goto(`${t.BASE}/`)
  await page.locator('input[type=text]').waitFor()
  await page.locator('input[type=text]').fill('admin')
  await page.locator('input[type=password]').fill(ADMIN_PW)
  await page.locator('button[type=submit]').click()
  t.check(
    await t.until(async () => (await page.locator('main').count()) > 0 && !(await page.locator('input[type=password]').count()), 10000),
    'admin signs in on the login page with the install password',
  )
  const who = await (await page.request.get(`${t.BASE}/api/v1/auth/session`)).json()
  t.check(who.user === 'admin' && who.roles.includes('admin'), 'as admin, an administrator', JSON.stringify(who))

  const nav = await (await page.request.get(`${t.BASE}/api/v1/console/nav`)).json()
  t.check(nav.some((s) => s.label === 'Admin' && s.items.some((i) => i.href === '#/users')), 'Admin → Users is in the navigator')

  t.check(await t.open(page, '#/users', /storm-users/), 'the Users page opens', await t.text(page))
  const text = await t.text(page)
  t.check(/admin/.test(text) && /cluster-admin/.test(text) && /built in/i.test(text), 'admin is listed, cluster-admin, built in', text)
  await t.shot(page, 'users-list')

  // A new storage-admin, from the form.
  await page.getByPlaceholder('e.g. storage1').fill('carol')
  const pw = page.locator('form input[type=password]')
  await pw.nth(0).fill('carol-pw')
  await pw.nth(1).fill('carol-pw')
  await page.getByRole('button', { name: 'Create', exact: true }).click()
  t.check(await t.until(async () => /User carol created/.test(await t.text(page)), 10000), 'carol is created from the form', await t.text(page))
  const rows = await (await page.request.get(`${t.BASE}/api/v1/users`)).json()
  const carol = rows.users.find((u) => u.name === 'carol')
  t.check(carol && carol.roles.join() === 'storage-admin' && carol.password, 'carol: storage-admin, a password', JSON.stringify(carol))
  await t.shot(page, 'users-created')

  // A group, from its form: carol in it, storage-admin (offered first).
  const gform = page.locator('form[aria-label="New group"]')
  await gform.getByPlaceholder('e.g. drives').fill('drives')
  await gform.getByPlaceholder('e.g. replaces and formats drives').fill('the drive team')
  await gform.locator('label', { hasText: 'carol' }).locator('input').check()
  await page.getByRole('button', { name: 'Create group' }).click()
  t.check(await t.until(async () => /Group drives created/.test(await t.text(page)), 10000), 'drives is created from the form', await t.text(page))
  const withGroup = await (await page.request.get(`${t.BASE}/api/v1/users`)).json()
  const drives = withGroup.groups.find((g) => g.name === 'drives')
  t.check(drives && drives.members.join() === 'carol' && drives.roles.join() === 'storage-admin' && drives.description === 'the drive team', 'drives: carol, storage-admin, its description', JSON.stringify(drives))
  const row = page.locator('table').first().locator('tr', { hasText: 'carol' })
  t.check(await t.until(async () => /drives/.test(await row.innerText())), "carol's row says she is in drives", await row.innerText())
  const grow = page.locator('table[aria-label="Groups"] tr', { hasText: 'drives' })
  t.check(/carol/.test(await grow.innerText()) && /storage-admin/.test(await grow.innerText()), 'the Groups table lists her and its role', await grow.innerText())
  await t.shot(page, 'users-group')

  // Her password, reset from the row: asked twice.
  page.answer = 'carol-pw2'
  await row.getByRole('button', { name: 'Reset password' }).click()
  t.check(await t.until(async () => /Password for carol set/.test(await t.text(page)), 10000), 'her password is reset from the row')

  // She signs in with the new one and is not offered Users.
  const her = await t.page('carol', 'carol-pw2')
  const herNav = await (await her.request.get(`${t.BASE}/api/v1/console/nav`)).json()
  t.check(!herNav.some((s) => s.items.some((i) => i.href === '#/users')), 'a storage-admin is not shown Users')
  const refused = await her.request.get(`${t.BASE}/api/v1/users`)
  t.check(refused.status() === 403, "a storage-admin is refused Users' API", `${refused.status()}`)

  // Delete, typing the name.
  page.answer = 'carol'
  await row.getByRole('button', { name: 'Delete' }).click()
  t.check(await t.until(async () => /carol deleted/.test(await t.text(page)), 10000), 'carol is deleted by typing her name')
  const gone = await (await page.request.get(`${t.BASE}/api/v1/users`)).json()
  t.check(!gone.users.some((u) => u.name === 'carol'), 'and is gone from the list')
  const after = await her.request.get(`${t.BASE}/api/v1/auth/session`)
  t.check(!(await after.json()).authenticated, 'her session ended with her')
  t.check(await t.until(async () => !/carol/.test(await grow.innerText())), 'and she is out of drives', await grow.innerText())

  // The group, deleted by typing its name; a wrong name deletes nothing.
  page.answer = 'drive'
  await grow.getByRole('button', { name: 'Delete' }).click()
  t.check(await t.until(async () => /Not deleted: that is not drives/.test(await t.text(page))), 'a wrong name deletes nothing')
  page.answer = 'drives'
  await grow.getByRole('button', { name: 'Delete' }).click()
  t.check(await t.until(async () => /Group drives deleted/.test(await t.text(page)), 10000), 'drives is deleted by typing its name')
  const none = await (await page.request.get(`${t.BASE}/api/v1/users`)).json()
  t.check(!none.groups.some((g) => g.name === 'drives'), 'and is gone')
})
