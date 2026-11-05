// The Update page in a browser (#164), over deploy/verify-update.sh's real stormupdate
// and apiserver. PHASE: offered, progress, failed, back.
const { walk } = require('./lib.cjs')
const PHASE = process.env.PHASE

walk(`update (${PHASE})`, async (t) => {
  const page = await t.page('root')
  const has = async (re) => t.until(async () => re.test(await t.text(page)))
  t.check(await t.open(page, '#/update', /Running/i), 'the page opens', (await t.text(page)).slice(0, 300))

  if (PHASE === 'offered') {
    const nav = await page.evaluate(() => fetch('/api/v1/console/nav').then((r) => r.json()))
    t.check(JSON.stringify(nav).includes('#/update'), 'Compute has an Update item')
    t.check(await has(/12\.58/), 'the running release')
    const offered = page.locator('section[aria-label="Offered"]')
    t.check(await t.until(async () => /12\.61/.test(await offered.innerText())), 'the offered release', await offered.innerText())
    t.check(/better things/.test(await offered.innerText()), 'with its notes')
    await t.shot(page, 'update-offered')

    // A reader sees it all and has nothing to press.
    const ro = await t.page('ops')
    await t.open(ro, '#/update', /12\.61/)
    t.check((await ro.getByRole('button', { name: /Update to/ }).count()) === 0, 'an operator is offered no Update')
    t.check(/administrator/.test(await t.text(ro)), 'and is told whose it is')

    // Dismissing the question sends nothing.
    const asked = page.locator('section[aria-label="Asked here"] tbody tr')
    const before = await asked.count()
    page.refuseNext = true
    await page.getByRole('button', { name: 'Update to 12.61' }).click()
    await t.pause(1500)
    t.check((await asked.count()) === before, 'dismissed, nothing is asked of stormupdate', `${before} then ${await asked.count()}`)
    // Accepting does.
    await page.getByRole('button', { name: 'Update to 12.61' }).click()
    t.check(await t.until(async () => (await asked.count()) === before + 1), 'accepted, one more act', `${before} then ${await asked.count()}`)
    t.check(/root\s+start 12\.61\s+done/.test(await asked.first().innerText()), 'done and recorded, with who', await asked.first().innerText())
    await t.shot(page, 'update-asked')
  }

  if (PHASE === 'progress') {
    t.check(await has(/Moving to\s*12\.61/i), 'what it is moving to', (await t.text(page)).slice(0, 300))
    t.check(await has(/waiting for fastetcd quorum/), "the cluster's step")
    t.check(await has(/this message for \d+/), 'and how long it has been there')
    const nodes = page.locator('section[aria-label="Nodes"]')
    t.check(/staging 12\.61: 3 of 7 volumes/.test(await nodes.innerText()), "the node's agent's own words", await nodes.innerText())
    t.check(/here for \d+m/.test(await nodes.innerText()), 'and its time there', await nodes.innerText())
    t.check((await nodes.locator('li[aria-current="step"]').first().innerText()) === 'Staging and rebooting', 'the step it is on is marked')
    t.check(/waiting on workloads, fastetcd/.test(await nodes.innerText()), 'a second node, on its gates')
    t.check(/Images of 12\.61/i.test(await t.text(page)) && /1 Pulled, 1 Pulling/.test(await t.text(page)), 'the images, per state')
    t.check((await page.getByRole('button', { name: 'Update to 12.61' }).isDisabled().catch(() => true)), 'no second update while one runs')
    await t.shot(page, 'update-progress')
    await page.getByRole('button', { name: 'Pause at the next node' }).click()
    t.check(await has(/Asked on this console[\s\S]*pause/i), 'pause asks first, and is recorded')
  }

  if (PHASE === 'failed') {
    t.check(await has(/refused: cannot read fastetcd quorum/), 'a refusal is said, with why')
    t.check(await has(/not kept here yet/), "and that it won't stay in the history")
  }

  if (PHASE === 'back') {
    t.check(await has(/Rolled back from\s*12\.61/i), 'the release rolled back from', (await t.text(page)).slice(0, 300))
    const h = page.locator('section[aria-label="History"]')
    t.check(/12\.60[\s\S]*12\.61[\s\S]*RolledBack[\s\S]*workloads did not come back/.test(await h.innerText()), 'history: the result and why', await h.innerText())
    t.check(/Completed/.test(await h.innerText()) && /1m 40s/.test(await h.innerText()), 'and the run before it, with how long it took', await h.innerText())
    await t.shot(page, 'update-back')
    await page.getByRole('button', { name: /Roll back to/ }).click()
    t.check(await has(/Asked on this console[\s\S]*rollback/i), 'rollback asks first, and is recorded')
  }
})
