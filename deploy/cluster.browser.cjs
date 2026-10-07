// The browser half of deploy/verify-cluster.sh (#63, #88): root forms a
// cluster on b1 — the plan in stormcluster's words, then the objects
// written — watches the `Cluster` object's status, joins b2 and releases it
// with a typed confirm; alice (no RBAC on cluster.storm.io) is shown the
// plan and then the apiserver's refusal. Any page error fails the run.
const { chromium } = require('playwright')

const BASE = process.env.CONSOLE
let failed = 0
const errors = []
function check(ok, what, extra = '') {
  console.log(`  ${ok ? 'ok  ' : 'FAIL'} ${what}${extra ? ` — ${extra}` : ''}`)
  if (!ok) failed++
}
const pause = (ms) => new Promise((r) => setTimeout(r, ms))
async function until(fn, ms = 20000) {
  const end = Date.now() + ms
  while (Date.now() < end) {
    try {
      if (await fn()) return true
    } catch {}
    await pause(500)
  }
  return false
}

async function as(browser, user) {
  const ctx = await browser.newContext({ viewport: { width: 1400, height: 1000 } })
  const page = await ctx.newPage()
  page.setDefaultTimeout(5000)
  page.on('pageerror', (e) => errors.push(`${user} pageerror: ${e.message.split('\n')[0]}`))
  page.on('console', (m) => {
    if (m.type() === 'error' && !/the server responded with a status of (409|401|403|404)/.test(m.text())) errors.push(`${user} console: ${m.text().split('\n')[0]}`)
  })
  const login = await ctx.request.post(`${BASE}/api/v1/auth/login`, { data: { username: user, password: 'pw' } })
  check(login.ok(), `signed in as ${user}`, `${login.status()}`)
  return page
}

;(async () => {
  const browser = await chromium.launch()
  const page = await as(browser, 'root')
  const plan = page.locator('[aria-label="The plan"]')
  const steps = () => plan.locator('ol.steps li').allInnerTexts()
  const outcome = page.locator('section.outcome')
  const body = () => page.locator('main').innerText()
  const row = (name) => page.locator('table.rows tr', { has: page.locator('.strong', { hasText: new RegExp(`^${name}$`) }) })
  const summary = page.locator('section.summary')

  try {
    console.log('\n--- 1. an SNO and its peers')
    await page.goto(`${BASE}/#/cluster`)
    check(await until(async () => (await body()).includes('b1 (SNO)')), 'the page names b1 as an SNO')
    for (const n of ['b2', 'b3']) check(await until(async () => (await row(n).count()) === 1), `${n} is listed as discovered`)
    check(await page.getByRole('button', { name: 'Resume' }).count() === 0, 'no Resume: a failed operation resumes by itself')

    console.log('\n--- 2. Form storm on b1: the plan, then the objects')
    await page.getByRole('button', { name: 'Form a cluster…' }).click()
    check((await page.locator('input[aria-label="Cluster name"]').inputValue()) === 'storm', 'the name is suggested')
    check(await page.locator('input[aria-label="b1 as master"]').isChecked() && await page.locator('input[aria-label="b1 as master"]').isDisabled(), 'b1 is the seed, and stays a master')
    await page.locator('input[aria-label="b2 as master"]').check()
    const show = page.getByRole('button', { name: 'Show the plan' })
    check(await show.isDisabled() && (await body()).includes('2 master(s): the control plane must be 1, 3 or 5'), 'two masters: no plan, and the page says why')
    await page.locator('input[aria-label="b2 as master"]').uncheck()
    await show.click()
    await plan.waitFor({ timeout: 15000 })
    const formSteps = await steps()
    check(formSteps.length > 0 && formSteps.every((s) => s.trim().length > 0), 'the plan is shown before anything is written', formSteps.join(' | '))
    check((await plan.innerText()).includes('Nothing is written yet'), 'and says nothing is written yet')
    await page.screenshot({ path: 'form-plan.png' })
    await plan.getByRole('button', { name: 'Write it' }).click()
    check(await until(async () => /wrote ClusterMember b1, Cluster storm/.test(await outcome.innerText())), 'Write it writes the member, then the Cluster', await outcome.innerText().catch(() => ''))
    check(await until(async () => /Cluster object\s+storm/.test(await summary.innerText()), 15000), 'the Cluster object is on the page', await summary.innerText().catch(() => ''))
    check(await until(async () => (await summary.locator('.phase').count()) > 0 && (await summary.locator('.phase').first().innerText()).length > 0, 30000),
      "with stormcluster's phase", await summary.locator('.objst').first().innerText().catch(() => ''))
    const formed = await until(async () => (await page.locator('h1').innerText()).startsWith('storm'), 60000)
    console.log(`    the cluster: ${await page.locator('h1').innerText()} · ${await summary.locator('.objst').first().innerText().catch(() => '')}`)
    await page.screenshot({ path: 'formed.png', fullPage: true })

    if (formed) {
      check(true, 'storm formed: the page is now the cluster')
      console.log('\n--- 3. Join b2 as a worker')
      await page.getByRole('button', { name: 'Join nodes…' }).click()
      await page.locator('input[aria-label="Join b2"]').check()
      await page.getByRole('button', { name: 'Show the plan' }).click()
      await plan.waitFor({ timeout: 15000 })
      check((await steps()).length > 0, 'the join plan', (await steps()).join(' | '))
      await plan.getByRole('button', { name: 'Write it' }).click()
      check(await until(async () => /wrote ClusterMember b2/.test(await outcome.innerText())), 'Write it writes ClusterMember b2', await outcome.innerText().catch(() => ''))
      check(await until(async () => (await row('b2').locator('.phase').count()) > 0, 30000), "b2's object status is on its row", await row('b2').first().innerText().catch(() => ''))
      // No stormcert here: the join fails or is blocked, in the object's words.
      const b2said = async () => row('b2').first().innerText()
      await until(async () => /Failed|Blocked|Ready|Joining/.test(await b2said()), 60000)
      console.log(`    b2: ${(await b2said()).replace(/\s+/g, ' ')}`)
      await page.screenshot({ path: 'joined.png', fullPage: true })

      console.log('\n--- 4. Release b2: a typed confirm')
      const rb = row('b2').first().getByRole('button', { name: /^(Release|Withdraw)$/ })
      await rb.click()
      await plan.waitFor({ timeout: 15000 })
      check((await plan.innerText()).includes('its data is erased'), 'the dialog says the data is erased', (await plan.innerText()).slice(0, 300))
      const write = plan.getByRole('button', { name: 'Write it' })
      check(await write.isDisabled(), 'Write it waits for the name')
      await plan.locator('input[aria-label="Confirm by typing b2"]').fill('b3')
      check(await write.isDisabled(), 'the wrong name does not do')
      await plan.locator('input[aria-label="Confirm by typing b2"]').fill('b2')
      await write.click()
      check(await until(async () => /ClusterMember b2 deleted/.test(await outcome.innerText())), 'released', await outcome.innerText().catch(() => ''))
      check(await until(async () => (await row('b2').count()) === 0 || /Releasing|Leaving/.test(await b2said()), 30000), 'b2 is going', await b2said().catch(() => 'gone'))
    } else {
      // Whatever stopped it is on the page, in stormcluster's words.
      const said = await summary.locator('.objst').first().innerText().catch(() => '')
      check(/Failed|Blocked/.test(said) && said.length > 10, 'the form did not finish here, and the Cluster object says why', said)
    }

    console.log('\n--- 5. alice: shown the plan, refused by the apiserver')
    const alice = await as(browser, 'alice')
    await alice.goto(`${BASE}/#/cluster`)
    const aliceBody = () => alice.locator('main').innerText()
    await until(async () => (await aliceBody()).includes('Nodes discovered') || (await aliceBody()).includes('Other nodes discovered'))
    const joinBtn = alice.getByRole('button', { name: 'Join nodes…' })
    if (await joinBtn.count()) {
      await joinBtn.click()
      await alice.locator('input[aria-label="Join b3"]').check()
    } else {
      await alice.getByRole('button', { name: 'Form a cluster…' }).click()
      await alice.locator('input[aria-label="b3 as worker"]').check()
    }
    await alice.getByRole('button', { name: 'Show the plan' }).click()
    const aplan = alice.locator('[aria-label="The plan"]')
    await aplan.waitFor({ timeout: 15000 })
    await aplan.getByRole('button', { name: 'Write it' }).click()
    const aout = alice.locator('section.outcome')
    check(await until(async () => /refused/.test(await aout.innerText()) && /forbidden|not allowed|cannot|403/i.test(await aout.innerText())), "alice gets the apiserver's refusal", await aout.innerText().catch(() => ''))
    await alice.screenshot({ path: 'alice.png', fullPage: true })
  } catch (e) {
    check(false, 'the run', e.message.split('\n')[0])
  }
  check(errors.length === 0, 'no page errors', errors.join(' | '))
  await browser.close()
  process.exit(failed ? 1 : 0)
})()
