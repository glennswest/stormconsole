// Admin → Users (#266): admin signs in on the login page with the install's
// password, opens Users from the navigator, makes a storage-admin, resets a
// password and deletes a user by typing the name. A storage-admin is not
// offered the page and is refused its API.
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
  await page.locator('form button[type=submit]').click()
  t.check(await t.until(async () => /User carol created/.test(await t.text(page)), 10000), 'carol is created from the form', await t.text(page))
  const rows = await (await page.request.get(`${t.BASE}/api/v1/users`)).json()
  const carol = rows.users.find((u) => u.name === 'carol')
  t.check(carol && carol.roles.join() === 'storage-admin' && carol.password, 'carol: storage-admin, a password', JSON.stringify(carol))
  await t.shot(page, 'users-created')

  // Her password, reset from the row: asked twice.
  page.answer = 'carol-pw2'
  const row = page.locator('tr', { hasText: 'carol' })
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
})
