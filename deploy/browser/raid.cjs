// The RAID sets page (#67), over verify-raid.sh's real engine: two bays failed and no spare left.
const { walk } = require('./lib.cjs')
const { WORD } = process.env

walk('raid sets', async (t) => {
  // bob may read, and is offered nothing to press on a member.
  const bob = await t.page('bob')
  t.check(await t.open(bob, '#/raid', /Shelf ds0/), 'bob sees the shelf', (await t.text(bob)).slice(0, 300))
  t.check((await bob.locator('[data-testid=member] button').count()) === 0, 'and no member button')
  t.check((await bob.getByRole('button', { name: 'Add a spare' }).count()) === 0, 'and no Add a spare')
  await t.shot(bob, 'raid-bob')

  const page = await t.page('alice')
  t.check(await t.open(page, '#/raid', /Shelf ds0/), 'alice sees the shelf', (await t.text(page)).slice(0, 300))
  const text = await t.text(page)
  t.check(/Degraded/.test(text), 'the set says degraded')
  t.check(/2 failed, survives 2/.test(text), 'and how many it has lost', text.slice(0, 400))
  t.check(/none: a failed member waits/.test(text), 'and that no spare is waiting')
  const failed = page.locator('[data-testid=member][data-state=failed]')
  t.check((await failed.count()) === 2, 'two bays marked failed')
  const marked = await failed.first().evaluate((e) => getComputedStyle(e).borderTopWidth)
  const plain = await page.locator('[data-testid=member][data-state=active]').first().evaluate((e) => getComputedStyle(e).borderTopWidth)
  t.check(marked !== plain, 'drawn apart from the healthy ones', `${marked} vs ${plain}`)
  t.check(/bay 4/.test(await failed.first().innerText()), 'by its bay', await failed.first().innerText())
  await t.shot(page, 'raid-degraded')

  const said = () => page.locator('[data-testid=said]').innerText().catch(() => '')

  // Replace a failed bay with a free drive. The guard wants a typed word.
  await failed.first().getByRole('button', { name: 'Replace' }).click()
  const pick = page.getByLabel('Replace with')
  const offered = await pick.locator('option').allInnerTexts()
  t.check(offered.some((o) => /bay8\.img/.test(o)) && offered.some((o) => /bay9\.img/.test(o)), 'the two free drives are offered', offered.join(', '))
  t.check(offered.every((o) => !/bay[1-5]\.img/.test(o)), 'and no member of the set', offered.join(', '))
  await t.shot(page, 'raid-replace')
  page.once('dialog', (d) => d.accept('not-it'))
  await page.getByRole('button', { name: 'Rebuild onto it' }).click()
  t.check(await t.until(async () => /not|type/i.test(await said())), 'the wrong word is refused, on the page', await said())
  // The drive a spare took over from is free to the API, and still carries its old superblock: the
  // engine says so, and it reaches the person.
  await pick.selectOption({ label: offered.find((o) => /bay6\.img/.test(o)) })
  page.once('dialog', (d) => d.accept(WORD))
  await page.getByRole('button', { name: 'Rebuild onto it' }).click()
  t.check(await t.until(async () => /force|superblock|carries|stale/i.test(await said())), "a drive the set replaced: the engine's refusal, on the page", await said())
  await pick.selectOption({ label: offered.find((o) => /bay8\.img/.test(o)) })
  page.once('dialog', (d) => d.accept(WORD))
  await page.getByRole('button', { name: 'Rebuild onto it' }).click()
  t.check(await t.until(async () => (await page.locator('[data-testid=rebuild]').count()) > 0), 'a rebuild starts and shows progress', await t.text(page))
  const line = page.locator('[data-testid=rebuild]').first()
  t.check(await t.until(async () => /Rebuilding slot/.test(await line.innerText())), 'naming the slot')
  t.check(/\d+%/.test(await line.innerText()), 'with its percent', await line.innerText())
  await t.shot(page, 'raid-rebuilding')

  // A spare, for the bay still failed.
  await page.getByRole('button', { name: 'Add a spare' }).click()
  const spare = page.getByLabel('Add as a spare')
  await spare.selectOption({ label: (await spare.locator('option').allInnerTexts()).find((o) => /bay9\.img/.test(o)) })
  page.once('dialog', (d) => d.accept('spares'))
  await page.getByRole('button', { name: 'Add it' }).click()
  t.check(await t.until(async () => /Spare added to ds0/.test(await said())), 'the spare is added', await said())
  await t.shot(page, 'raid-spare')
})
