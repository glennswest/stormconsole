// Machines in a browser (#58, for #31), over deploy/verify-machines.sh's
// stormipmi on ipmi_sim: power, the release a machine boots, its boot
// intent, the SOL console — as admin, then as ops (shown, not offered).
const { walk } = require('./lib.cjs')
const TAG = process.env.TAG || 'SIMBOARD0001'

walk('machines', async (t) => {
  const page = await t.page('admin')
  const row = () => page.locator('table tr', { hasText: TAG })
  t.check(await t.open(page, '#/machines', new RegExp(TAG)), `the page lists ${TAG}`, (await t.text(page)).slice(0, 200))
  t.check(/Default image/i.test(await t.text(page)), 'with the default image')
  await t.shot(page, 'machines')

  // Power: soft off, then on — each confirmed, each read back.
  await page.locator(`select[aria-label="Power ${TAG}"]`).selectOption('soft')
  t.check(await t.until(async () => /\boff\b/i.test(await row().first().innerText()), 30000), 'Shut down (ACPI): the row reads off', await row().first().innerText())
  await page.locator(`select[aria-label="Power ${TAG}"]`).selectOption('on')
  t.check(await t.until(async () => /\bon\b/i.test(await row().first().innerText()), 30000), 'Power on: the row reads on', await row().first().innerText())

  // The release it boots: set, confirmed, read back.
  const rel = page.locator(`select[aria-label="Release for ${TAG}"]`)
  const options = await rel.locator('option').allInnerTexts()
  const current = (await row().first().innerText()).match(/\b(\d+\.\d+)\b/)?.[1]
  const next = options.map((o) => o.split(' ')[0]).find((v) => /^\d/.test(v) && v !== current)
  t.check(!!next, 'there is another release to point it at', options.join(', '))
  if (next) {
    await rel.selectOption(next)
    t.check(await t.until(async () => (await row().first().innerText()).includes(next), 20000), `Set release ${next}: read back`, await row().first().innerText())
  }
  await row().first().getByRole('button', { name: 'boot intent' }).click()
  t.check(await t.until(async () => /intent:/.test(await row().first().innerText())), 'boot intent answers (as stormipmi words it)', await row().first().innerText())

  // The SOL console: replay and live output, and what is typed echoes.
  await row().first().getByRole('button', { name: 'Console' }).click()
  const screen = page.locator('section.console pre.screen')
  t.check(await t.until(async () => (await screen.innerText()).length > 0, 20000), 'the console shows the replay', (await screen.innerText()).slice(-200))
  await page.locator('input[aria-label="Console input"]').fill('typed in the browser')
  await page.locator('input[aria-label="Console input"]').press('Enter')
  t.check(await t.until(async () => /typed in the browser/.test(await screen.innerText()), 20000), 'typing echoes back', (await screen.innerText()).slice(-200))
  await t.shot(page, 'machines-console')
  await page.locator('section.console').getByRole('button', { name: 'Close' }).click()

  // ops: everything shown, nothing that changes a machine offered.
  const ops = await t.page('ops')
  t.check(await t.open(ops, '#/machines', new RegExp(TAG)), 'ops sees the machines')
  t.check((await ops.locator(`select[aria-label="Power ${TAG}"]`).count()) === 0, 'ops is offered no power')
  t.check((await ops.locator(`select[aria-label="Release for ${TAG}"]`).count()) === 0, 'nor a release')
  await ops.locator('table tr', { hasText: TAG }).first().getByRole('button', { name: 'Console' }).click()
  t.check(await t.until(async () => /watching — typing is for administrators/.test(await ops.locator('section.console').innerText())), 'ops watches the console, and is told why there is no input')
  t.check((await ops.locator('input[aria-label="Console input"]').count()) === 0, 'and has no input')
})
