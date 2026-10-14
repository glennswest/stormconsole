// The login page's token mode (#66): the install-time API token signs in
// with no user name — on a console with only that token, the first screen;
// beside named users, one click away.
const { walk } = require('./lib.cjs')
const TOKEN = process.env.TOKEN
const ALONE = process.env.ALONE

// On the token screen already.
async function signIn(t, page, base, label) {
  await page.locator('input[type=password]').waitFor()
  t.check((await page.locator('input[type=text]').count()) === 0, `${label}: one field, no user name`)
  const body = await page.locator('body').innerText()
  t.check(/API token/i.test(body), `${label}: the page names the API token`, body)
  t.check(/apiToken/.test(body) && /storminstall wait/.test(body), `${label}: and says where it comes from`)
  await page.locator('input[type=password]').fill('not-it')
  await page.locator('button[type=submit]').click()
  t.check(await t.until(async () => /invalid credentials/i.test(await page.locator('body').innerText()), 5000), `${label}: a wrong token is refused on the page`)
  await page.locator('input[type=password]').fill(TOKEN)
  await page.locator('button[type=submit]').click()
  const inside = await t.until(async () => (await page.locator('main').count()) > 0 && !(await page.locator('input[type=password]').count()), 10000)
  t.check(inside, `${label}: the token opens the console`)
  const who = await (await page.request.get(`${base}/api/v1/auth/session`)).json()
  t.check(who.user === 'system:admin' && who.roles.includes('admin'), `${label}: signed in as system:admin, admin`, JSON.stringify(who))
  await t.shot(page, `${label}-inside`)
}

walk('install token', async (t) => {
  // Only the install token: the token mode is the first screen, with no switch.
  const alone = await t.page()
  await alone.goto(`${ALONE}/`)
  await alone.locator('input[type=password]').waitFor()
  await t.shot(alone, 'alone-login')
  t.check(!/instead/.test(await alone.locator('body').innerText()), 'alone: no switch to a user name')
  await signIn(t, alone, ALONE, 'alone')

  // Beside a named user: user name first, the token one click away.
  const both = await t.page()
  await both.goto(`${t.BASE}/`)
  await both.locator('input[type=text]').waitFor()
  t.check((await both.locator('input[type=text]').count()) === 1, 'both: the user name screen first')
  await both.getByRole('button', { name: /API token instead/ }).click()
  await t.shot(both, 'both-token-mode')
  await signIn(t, both, t.BASE, 'both')
})
