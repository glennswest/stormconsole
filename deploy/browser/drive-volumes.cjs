// The volumes on every node's drives (#52), over deploy/verify-drive-volumes.sh. PHASE: root, alice.
const { walk } = require('./lib.cjs')
const PHASE = process.env.PHASE

walk(`drive volumes (${PHASE})`, async (t) => {
  const page = await t.page(PHASE)
  const pick = async (serial) => {
    await page.locator('input[aria-label="Search drives"]').fill(serial)
    t.check(await t.until(async () => (await page.locator('button.bay').count()) === 1), `search finds ${serial}`)
    await page.locator('button.bay').first().click()
    const p = page.locator('section.picked')
    t.check(await t.until(async () => /Volumes on this drive/i.test(await p.innerText())), `${serial} opens`, await p.innerText().catch(() => ''))
    return p
  }
  t.check(await t.open(page, '#/drives', /FAR-1|drive/), 'the drives page')
  t.check(await t.until(async () => (await page.locator('button.bay').count()) === 6), 'every node\'s drives', `${await page.locator('button.bay').count()}`)

  let p = await pick('FAR-1')
  const row = (name) => p.locator('tr', { hasText: name })
  if (PHASE === 'alice') {
    t.check(/shop\/db/.test(await row('pvc-far').innerText()), 'another node: her project\'s consumer', await row('pvc-far').innerText())
    t.check(/something in a project you cannot see/.test(await row('vm-sec').innerText()), 'the other project\'s: said to exist', await row('vm-sec').innerText())
    t.check(!/vault/.test(await page.content()), 'and not named anywhere in the page')
    t.check((await row('vm-sec').getByRole('link').count()) === 0, 'and no link to it')
    await t.shot(page, 'drive-volumes-alice')
    return
  }
  const text = await p.innerText()
  t.check(/pvc-far/.test(text) && /PersistentVolumeClaim shop\/db/.test(text), 'another node\'s volume and its consumer', text.slice(0, 600))
  t.check(/draining/.test(await row('pvc-far').innerText()) && /rebuild running/.test(await row('pvc-far').innerText()), 'its state', await row('pvc-far').innerText())
  t.check(/shop\/vault|secret\/vault/.test(text), 'an administrator sees every project')
  t.check(/nothing/.test(await row('scratch').innerText()), 'a volume nothing uses')
  t.check((await row('pvc-far').getByRole('link', { name: 'pvc-far' }).count()) === 0, 'its name has no row here to lead to')
  await t.shot(page, 'drive-volumes')
  await row('pvc-far').getByRole('link', { name: /shop\/db/ }).click()
  t.check(await t.until(async () => /\bdb\b/.test(await t.text(page)) && !/Search drives/.test(await page.content())), 'its consumer leads to the claim', (await t.text(page)).slice(0, 300))

  t.check(await t.open(page, '#/drives', /drive/), 'back')
  p = await pick('FAR-EMPTY')
  t.check(/No volume has data on this drive/.test(await p.innerText()), 'an empty list is an answer')
  p = await pick('FAR-STALE')
  t.check(/As n2’s stormdrive last heard from its engine, .*ago/.test(await p.innerText()), 'an old answer says how old', await p.innerText())
  p = await pick('OLD-1')
  t.check(/n3’s stormdrive does not report its own/.test(await p.innerText()), 'a node that does not report is not called empty', await p.innerText())
  t.check(!/No volume has data/.test(await p.innerText()), 'and not said to have none')
  p = await pick('HERE-1')
  t.check(/slab-HERE-1 draining/.test(await p.innerText()), 'this node: a slab\'s state from the engine, beside stormdrive\'s list', (await p.innerText()).slice(0, 500))
  t.check((await p.getByRole('link', { name: 'pvc-local' }).count()) === 1, 'and its volume has a row to lead to')
  p = await pick('HERE-2')
  t.check(/engine-says/.test(await p.innerText()), 'a stormdrive that does not report: this engine, the fallback', (await p.innerText()).slice(0, 500))
})
