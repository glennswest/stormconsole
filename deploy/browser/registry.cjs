// The Registry page in a browser (#163), over deploy/verify-registry.sh's real engine
// and registry: repositories and tags, who holds each clone, the filters with counts,
// the reverse view, and a viewer who may not see another project's holder.
const { walk } = require('./lib.cjs')
const { BARE } = process.env

walk('registry', async (t) => {
  const page = await t.page('root')
  t.check(await t.open(page, '#/registry', /stormlb/), 'the page opens', (await t.text(page)).slice(0, 400))
  let text = await t.text(page)
  t.check(/\d+ repositories/.test(text) && /\d+ tags/.test(text) && /\d+ clones/.test(text), 'it counts repositories, tags and clones', text.slice(0, 300))
  t.check(!/golden/i.test(text), 'the word is not on the page', (text.match(/.{0,40}golden.{0,40}/i) || [''])[0])
  const nav = await page.evaluate(() => fetch('/api/v1/console/nav').then((r) => r.json()))
  t.check(/#\/registry/.test(JSON.stringify(nav)), 'Images has a Registry item')

  const repo = page.locator('section.card', { hasText: 'stormlb' }).first()
  const r = await repo.innerText()
  t.check(/aaaaaaaaaaaa/.test(r) && /bbbbbbbbbbbb/.test(r), 'the repository lists its tags', r.slice(0, 300))
  for (const [word, who] of [['BOOT HOST|Boot host', 'r230'], ['Pod', 'shop/web'], ['Claim', 'shop/db'], ['VM', 'lab/vm1'], ['Release disk', '12.61'], ['Build', 'build-77'], ['Attached', 'nvme-tcp'], ['Nobody', 'nothing is serving it']]) {
    t.check(new RegExp(`(${word})[\\s\\S]{0,60}${who.replace('.', '\\.')}`, 'i').test(r), `${who} is shown as ${word.split('|').pop()}`, r.slice(0, 200))
  }
  t.check(/claimed \d+[smhd] ago/.test(r), 'a boot host says when it last claimed')
  await t.shot(page, 'registry-repositories')

  // Orphans only, with the count on the button.
  const chips = page.getByRole('group', { name: 'Held by' })
  const nobody = chips.getByRole('button', { name: /Nobody/ })
  t.check(/Nobody\s*\d+/.test(await nobody.innerText()), 'the filter carries its count', await nobody.innerText())
  await nobody.click()
  t.check(await t.until(async () => !/shop\/web/.test(await t.text(page))), 'orphans only leaves the holders out')
  t.check(/clone-lost/.test(await t.text(page)), 'and keeps the orphan')
  await chips.getByRole('button', { name: /^All/ }).click()

  // Pick a machine: which tag does it run on.
  await page.getByRole('button', { name: 'By holder' }).click()
  t.check(await t.until(async () => /Runs on/i.test(await t.text(page))), 'the reverse view opens')
  await page.getByLabel('Search').fill('r230')
  t.check(await t.until(async () => (await page.locator('table.flat tbody tr').count()) === 1), 'one row for the machine', await t.text(page))
  t.check(/stormlb:aaaaaaaaaaaa/.test(await page.locator('table.flat tbody tr').innerText()), 'it runs on that tag, through its own image', await page.locator('table.flat tbody tr').innerText())
  await page.getByLabel('Search').fill('shop/web')
  await page.getByLabel('Search').fill('web')
  t.check(await t.until(async () => /stormlb:aaaaaaaaaaaa/.test(await page.locator('table.flat').innerText())), 'and a pod')
  await t.shot(page, 'registry-holders')

  // alice has no project: another project's holder is counted and not named.
  const alice = await t.page('alice')
  t.check(await t.open(alice, '#/registry', /stormlb/), 'alice sees the registry')
  const a = await t.text(alice)
  t.check(!/shop\/web|shop\/db|lab\/vm1/.test(a), "she is not told another project's holders", a.slice(0, 400))
  t.check(/in a project you cannot see/.test(a), 'she is told they are there')
  t.check(/r230/.test(a), 'the machines are not a project\'s')

  // No engine token, or no engine: said in words.
  const bare = await t.page()
  await bare.goto(`${BARE}/#/registry`)
  t.check(await t.until(async () => /could not be read/.test(await t.text(bare))), 'an engine that refuses is said', (await t.text(bare)).slice(0, 300))
  t.check(/401/.test(await t.text(bare)), 'with its answer')
})
