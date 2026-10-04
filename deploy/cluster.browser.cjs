// The browser half of deploy/verify-cluster.sh (#63): an administrator on
// the Cluster page forming a cluster on b1, joining b2 (which fails where a
// stormcert would be, and is resumed), then — with a three-member record
// published to b1 — a refusal shown as its reasons, Promote-in-pairs, Split
// with keep or wipe, and a Drain run to its failure. Then an operator, who
// is shown everything and offered nothing. Any page error fails the run.
const { chromium } = require('playwright')

const BASE = process.env.CONSOLE
const B1 = process.env.B1
const TOKEN = process.env.TOKEN
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

;(async () => {
  const browser = await chromium.launch()
  const ctx = await browser.newContext({ viewport: { width: 1400, height: 1000 } })
  const page = await ctx.newPage()
  page.setDefaultTimeout(5000)
  page.on('pageerror', (e) => errors.push(`pageerror: ${e.message.split('\n')[0]}`))
  page.on('response', (r) => {
    if (r.status() === 404) errors.push(`404: ${r.url()}`)
  })
  page.on('console', (m) => {
    if (m.type() === 'error' && !/the server responded with a status of (409|401|403|404)/.test(m.text())) errors.push(`console: ${m.text().split('\n')[0]}`)
  })
  const login = await ctx.request.post(`${BASE}/api/v1/auth/login`, { data: { username: 'admin', password: 'pw' } })
  check(login.ok(), 'signed in as admin', `${login.status()}`)

  const plan = page.locator('[aria-label="The plan"]')
  const steps = () => plan.locator('ol.steps li').allInnerTexts()
  const row = (table, name) => page.locator(`table.rows tr`, { has: page.locator('.strong', { hasText: new RegExp(`^${name}$`) }) })
  const outcome = page.locator('section.outcome')
  const body = () => page.locator('main').innerText()

  try {
    console.log('\n--- 1. an SNO and its peers')
    await page.goto(`${BASE}/#/cluster`)
    check(await until(async () => (await body()).includes('b1 (SNO)')), 'the page names b1 as an SNO')
    for (const n of ['b1', 'b2', 'b3']) check(await row('peers', n).count() === 1, `${n} is listed as discovered`)
    check(await page.getByRole('button', { name: 'Form here' }).count() === 0 && await row('', 'b2').getByRole('button', { name: 'Form a cluster here' }).count() === 1, 'each peer offers Form a cluster here')

    console.log('\n--- 2. Form a cluster: one master, b1')
    await page.getByRole('button', { name: 'Form a cluster…' }).click()
    const name = page.locator('input[aria-label="Cluster name"]')
    check((await name.inputValue()) === 'storm', 'the name is suggested', await name.inputValue())
    check(await page.locator('input[aria-label="b1 as master"]').isChecked(), 'this node is the first master')
    await page.locator('input[aria-label="b2 as master"]').check()
    const show = page.getByRole('button', { name: 'Show the plan' })
    check(await show.isDisabled(), 'two masters: the plan cannot be asked for')
    check((await body()).includes('2 master(s): the control plane must be 1, 3 or 5'), 'and the page says why')
    await page.locator('input[aria-label="b2 as master"]').uncheck()
    await show.click()
    await plan.waitFor({ timeout: 10000 })
    const formSteps = await steps()
    check(formSteps.length === 4 && formSteps[1] === 'seed cluster storm on b1', 'the plan is shown before anything runs', formSteps.join(' | '))
    await page.screenshot({ path: 'form-plan.png' })
    await plan.getByRole('button', { name: /^Run 4 steps$/ }).click()
    check(await until(async () => /started form-/.test(await outcome.innerText())), 'Run starts it', await outcome.innerText().catch(() => ''))
    check(await until(async () => (await body()).includes('Members') && (await row('', 'b1').innerText()).includes('cluster CA'), 25000), 'b1 is a member, with the cluster CA')
    check(await until(async () => (await page.locator('h1').innerText()).startsWith('storm')), 'the page is now the cluster storm', await page.locator('h1').innerText())
    const formRow = page.locator('table.rows tr:not(.opsteps)', { hasText: 'form b1' })
    check(await until(async () => (await formRow.innerText()).includes('4/4')), 'the form operation finished its 4 steps', await formRow.innerText().catch(() => ''))
    await formRow.getByRole('button', { name: 'Steps' }).click()
    check(await until(async () => (await page.locator('tr.opsteps').first().textContent()).includes('done')), 'its steps are listed as done')

    console.log('\n--- 3. Join b2 as a worker: it fails at the token, and is resumable')
    await row('', 'b2').getByRole('button', { name: 'Join as worker' }).click()
    await plan.waitFor({ timeout: 10000 })
    const joinSteps = await steps()
    check(joinSteps.includes('issue a join token for b2'), 'the join plan', joinSteps.join(' | '))
    await plan.getByRole('button', { name: /^Run \d+ steps?$/ }).click()
    const joinRow = page.locator('table.rows tr:not(.opsteps)', { hasText: 'join b2' })
    const t0 = Date.now()
    check(await until(async () => (await joinRow.getAttribute('class')).includes('health-error'), 120000), 'the join failed (no stormcert here)', `${Math.round((Date.now() - t0) / 1000)} s: ${await joinRow.innerText().catch(() => '')}`)
    const resume = joinRow.getByRole('button', { name: 'Resume' })
    check(await until(() => resume.isEnabled()), 'Resume is offered')
    await joinRow.getByRole('button', { name: 'Steps' }).click()
    check(await until(async () => (await page.locator('tr.opsteps .st-failed').count()) > 0), 'the failed step is shown with its error',
      await page.locator('tr.opsteps .st-failed').first().innerText().catch(() => ''))
    await resume.click()
    check(await until(async () => /started join-/.test(await outcome.innerText())), 'Resume runs it again', await outcome.innerText().catch(() => ''))
    await page.screenshot({ path: 'joined.png', fullPage: true })

    console.log('\n--- 4. three members: published to b1 as its coordinator would')
    const rec = await (await ctx.request.get(`${B1}/api/v1/cluster`)).json()
    const now = new Date().toISOString()
    rec.generation += 1
    rec.members = rec.members.filter((m) => m.node === 'b1')
    rec.members.push({ node: 'b2', addr: '127.0.0.12', role: 'worker', state: 'ready', joinedAt: now })
    rec.members.push({ node: 'b3', addr: '127.0.0.13', role: 'worker', state: 'ready', joinedAt: now })
    const put = await ctx.request.put(`${B1}/api/v1/record`, { data: rec, headers: { Authorization: `Bearer ${TOKEN}` } })
    check(put.status() === 204, 'b1 took the record', `${put.status()}`)
    check(await until(async () => (await row('', 'b3').innerText()).includes('worker')), 'b2 and b3 are listed as workers')
    check(await row('', 'b1').getByRole('button', { name: 'Demote' }).isDisabled(), "the seed's Demote is not offered")

    console.log('\n--- 5. a refusal: promoting one worker of a one-master cluster')
    await row('', 'b2').getByRole('button', { name: 'Promote' }).click()
    check(await until(async () => (await outcome.getAttribute('class')).includes('bad')), 'refused')
    const reasons = await outcome.locator('ul.reasons li').allInnerTexts()
    check(reasons.some((r) => r.includes('2 masters: the control plane must be odd')), 'every reason, listed', reasons.join(' | '))
    check(await plan.count() === 0, 'and no plan is offered to run')
    await page.screenshot({ path: 'refused.png' })

    console.log('\n--- 6. Promote workers: in pairs')
    await page.getByRole('button', { name: 'Promote workers…' }).click()
    await page.locator('input[aria-label="Promote b2"]').check()
    check(await show.isDisabled() && (await body()).includes('1 + 1 masters is even: promote in pairs'), 'one is refused on the page, with why')
    await page.locator('input[aria-label="Promote b3"]').check()
    await show.click()
    await plan.waitFor({ timeout: 10000 })
    const pSteps = await steps()
    check(pSteps.some((s) => s.includes('b2')) && pSteps.some((s) => s.includes('b3')), 'a pair is planned', pSteps.join(' | '))
    await plan.getByRole('button', { name: 'Cancel' }).click()
    await page.getByRole('button', { name: 'Cancel' }).click()

    console.log('\n--- 7. Split b3: keep or wipe its data')
    await row('', 'b3').getByRole('button', { name: 'Split to SNO' }).click()
    await plan.waitFor({ timeout: 10000 })
    check((await steps()).includes('revert b3 to SNO (keeping its data)'), 'keeping its data by default', (await steps()).join(' | '))
    await plan.getByLabel('wipe its data').check()
    check(await until(async () => (await steps()).includes('revert b3 to SNO (wiping its data)')), 'wiping is planned again', (await steps()).join(' | '))
    await page.screenshot({ path: 'split.png' })
    await plan.getByRole('button', { name: 'Cancel' }).click()
    check(await until(async () => (await plan.count()) === 0), 'Cancel runs nothing')

    console.log('\n--- 8. Drain b2: run, and it fails where the apiserver would be')
    await row('', 'b2').getByRole('button', { name: 'Drain' }).click()
    await plan.waitFor({ timeout: 10000 })
    check((await steps()).join('|') === 'cordon b2|evict the pods on b2', 'cordon, then evict', (await steps()).join(' | '))
    await plan.getByRole('button', { name: /^Run 2 steps$/ }).click()
    const drainRow = page.locator('table.rows tr:not(.opsteps)', { hasText: 'drain b2' })
    check(await until(async () => (await drainRow.getAttribute('class')).includes('health-error'), 120000), 'drain failed, with its error', await drainRow.innerText().catch(() => ''))
    await page.screenshot({ path: 'cluster.png', fullPage: true })
  } catch (e) {
    check(false, 'the admin walk finished', e.message.split('\n')[0])
  }

  try {
    console.log('\n--- 9. an operator: everything shown, nothing offered')
    const octx = await browser.newContext()
    const op = await octx.newPage()
    op.on('pageerror', (e) => errors.push(`pageerror: ${e.message.split('\n')[0]}`))
    await octx.request.post(`${BASE}/api/v1/auth/login`, { data: { username: 'ops', password: 'pw' } })
    await op.goto(`${BASE}/#/cluster`)
    check(await until(async () => (await op.locator('main').innerText()).includes('Members')), 'ops sees the members')
    const text = await op.locator('main').innerText()
    check(text.includes('are for administrators'), 'and is told why there are no buttons')
    check(await op.locator('table.rows .acts button', { hasText: /Promote|Demote|Drain|Split|Join|Resume/ }).count() === 0, 'no action buttons')
    await octx.close()
  } catch (e) {
    check(false, 'the operator walk finished', e.message.split('\n')[0])
  }

  check(errors.length === 0, 'no page errors', errors.slice(0, 5).join(' | '))
  await browser.close()
  console.log(failed ? `\n${failed} failed` : '\nbrowser: all passed')
  process.exit(failed ? 1 : 0)
})()
