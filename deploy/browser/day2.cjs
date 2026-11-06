// Scale, cordon and drain in a browser (#36), over verify-day2.sh's real apiserver.
const { walk } = require('./lib.cjs')
const { GUARD } = process.env

walk('day 2', async (t) => {
  const bob = await t.page('bob')
  t.check(await t.open(bob, '#/node/n1', /Workloads on this node/), "bob sees the node's plan", (await t.text(bob)).slice(0, 300))
  const bobCard = bob.locator('section[aria-label="Drain"]')
  t.check((await bobCard.getByRole('button', { name: /Drain|Cordon|Uncordon/ }).count()) === 0, 'and no button')
  t.check(/admin role/.test(await bobCard.innerText()), 'and is told why')

  const page = await t.page('alice')

  // The number on the row.
  t.check(await t.open(page, '#/k8s/deploy', /web/), 'Deployments lists web', (await t.text(page)).slice(0, 300))
  const field = page.getByLabel('Replicas for web')
  t.check((await field.inputValue()) === '3', 'its number is what it has', await field.inputValue())
  const row = page.locator('tr', { has: field })
  t.check(await row.getByRole('button', { name: 'Scale' }).isDisabled(), 'Scale is off until it changes')
  await field.fill('4')
  await row.getByRole('button', { name: 'Scale' }).click()
  t.check(await t.until(async () => /web: 4 replicas/.test(await t.text(page))), 'the row scales and says so', (await t.text(page)).slice(0, 300))
  await t.shot(page, 'day2-scale')

  // The node's page.
  t.check(await t.open(page, '#/node/n1', /Workloads on this node/), "alice sees n1's plan")
  const card = page.locator('section[aria-label="Drain"]')
  t.check(/Cordoned/.test(await card.innerText()), 'it reads cordoned, after the drain', (await card.innerText()).slice(0, 300))
  t.check(new RegExp(GUARD).test(await page.locator('[data-testid=evict]').innerText()), 'the guarded pod is still to go')
  t.check(/solo/.test(await page.locator('[data-testid=unmanaged]').innerText()), 'the pod with no controller is apart')
  t.check(/ds-agent/.test(await page.locator('[data-testid=stays]').innerText()) && /mirror-n1/.test(await page.locator('[data-testid=stays]').innerText()), 'the DaemonSet and mirror pods stay')
  await t.shot(page, 'day2-plan')

  // Drain again: refused by the budget, on the page.
  await card.getByRole('button', { name: 'Drain' }).click()
  t.check(await t.until(async () => (await page.locator('[data-testid=blocked]').count()) > 0), "the budget's refusal is on the page", (await card.innerText()).slice(0, 400))
  t.check(/disruption budget/.test(await page.locator('[data-testid=blocked]').innerText()), 'in its words', await page.locator('[data-testid=blocked]').innerText())
  await t.shot(page, 'day2-blocked')

  // Ask for the unmanaged one too.
  await page.getByLabel('Evict these too').check()
  await card.getByRole('button', { name: 'Drain' }).click()
  t.check(await t.until(async () => !/solo/.test(await card.innerText())), 'asked for, the pod with no controller goes', (await card.innerText()).slice(0, 400))

  await card.getByRole('button', { name: 'Uncordon' }).click()
  t.check(await t.until(async () => /Schedulable/.test(await card.innerText())), 'Uncordon')
  t.check((await card.getByRole('button', { name: 'Cordon' }).count()) === 1, 'and Cordon is offered again')
  await t.shot(page, 'day2-uncordoned')

  // The opened row's plain button has no number to send, so it asks.
  t.check(await t.open(page, '#/k8s/deploy', /theirs/), 'back on Deployments')
  const theirs = page.locator('tr', { hasText: 'theirs' }).first()
  await theirs.getByRole('button', { name: 'Expand' }).click()
  page.answer = '2'
  await page.locator('.all-acts').getByRole('button', { name: 'Scale' }).click()
  t.check(await t.until(async () => /theirs: 2 replicas/.test(await t.text(page))), 'a plain button asks for the number', (await t.text(page)).slice(0, 300))
  page.answer = undefined

  // A card has only that button.
  await page.getByTitle('Card view').click()
  t.check(await t.until(async () => (await page.locator('.sc-cards').count()) > 0), 'the card view')
  await t.shot(page, 'day2-cards')
})
  const opened = page.locator('.full-actions button, .expanded button, [class*=expanded] button', { hasText: /^Scale$/ })
  if (await opened.count()) await opened.first().click()
  else await page.evaluate(() => {})
})
