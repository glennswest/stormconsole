// Network Flows (#4), over deploy/verify-flows.sh's real apiserver and a stand-in relay. PHASE: root, alice, none.
const { walk } = require('./lib.cjs')
const { PHASE } = process.env

walk(`flows (${PHASE})`, async (t) => {
  const page = await t.page(PHASE === 'none' ? 'root' : PHASE)
  const box = page.locator('[data-testid=flows]')
  await page.goto(`${t.BASE}/#/network/flows`)
  if (PHASE === 'none') {
    t.check(await t.until(async () => /Hubble relay did not answer/.test(await box.innerText())), 'no relay: said', await box.innerText().catch(() => ''))
    return
  }
  t.check(await t.until(async () => /shop\/web/.test(await box.innerText())), 'the flows are listed', await box.innerText().catch(() => ''))
  const text = () => box.innerText()
  t.check(/dropped/.test(await text()) && /allowed/.test(await text()), 'with their verdicts', await text())
  t.check(/policy denied/.test(await text()), "and a drop's reason", await text())
  t.check(/TCP\/5432/.test(await text()), 'and the port', await text())
  t.check(/world 203\.0\.113\.9/.test(await text()), 'the outside world is named as that', await text())
  if (PHASE === 'alice') {
    t.check(/another project/.test(await text()), "the other project's end is said to be one", await text())
    const html = await page.locator('main').innerHTML()
    t.check(!/vault|secret|keys|10\.0\.0\.3/.test(html), 'and nothing about it is in the page', html.slice(0, 300))
    await t.shot(page, 'flows-alice')
    return
  }
  t.check(/secret\/vault/.test(await text()), 'an administrator sees every project', await text())
  await page.locator('select[aria-label=Verdict]').selectOption('DROPPED')
  t.check(await t.until(async () => !/shop\/db/.test(await text()) && /dropped/.test(await text())), 'the verdict filter', await text())
  await page.locator('select[aria-label=Verdict]').selectOption('')
  await t.until(async () => /shop\/db/.test(await text()))
  const count = async () => parseInt((await page.locator('[data-testid=flows] .count').innerText()).match(/\d+/)[0])
  const before = await count()
  await page.locator('input[aria-label=Follow]').check()
  t.check(await t.until(async () => (await count()) > before + 1, 20000), 'Follow adds flows as they happen', `${before} ${await count()}`)
  await page.locator('input[aria-label=Follow]').uncheck()
  await t.shot(page, 'flows-root')

  t.check(await t.open(page, '#/pod/shop/web', /Overview/), 'the pod page')
  await page.locator('nav.tabs button', { hasText: 'Network' }).click()
  const pod = page.locator('[aria-label=Flows] [data-testid=flows]')
  t.check(await t.until(async () => /shop\/web/.test(await pod.innerText())), "its Network tab has the pod's flows", await pod.innerText().catch(() => ''))
  t.check(!/shop\/web-2/.test(await pod.innerText()), 'its own, not a pod whose name starts the same', await pod.innerText())
  await t.shot(page, 'flows-pod')

  t.check(await t.open(page, '#/vm/shop/vm1', /Machine|State/), "a machine's page")
  const vm = page.locator('.card[aria-label=Flows]')
  t.check(await t.until(async () => /not on the pod network|flow/i.test(await vm.innerText())), 'has flows, or says why it has none', await vm.innerText().catch(() => ''))
  await t.shot(page, 'flows-vm')
})
