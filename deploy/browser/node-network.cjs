// A node's Network page (#336), over deploy/verify-node-network.sh. PHASE: reader, admin, charts.
// HOST is the node (this console's own), PORTNAME the 25G port's NetworkPort.
const { walk } = require('./lib.cjs')
const { PHASE, HOST, PORTNAME } = process.env
const WHO = { reader: 'reader', admin: 'carol', charts: 'carol' }[PHASE]

walk(`node network (${PHASE})`, async (t) => {
  const page = await t.page(WHO)
  const text = () => t.text(page)
  const has = (re, ms) => t.until(async () => re.test(await text()), ms)
  const cell = (attr) => page.locator(`[${attr}]`).first().innerText().catch(() => '')

  if (PHASE === 'charts') {
    t.check(await t.open(page, `#/node/${HOST}/network?port=${PORTNAME}`, /Over time/, 20000), 'the port page')
    t.check(await has(/FEC blocks/, 20000), 'its charts', (await text()).slice(-600))
    t.check(await t.until(async () => (await page.locator('section.charts svg').count()) > 0, 20000), 'drawn')
    t.check(!/ironprom did not answer|refused this chart/.test(await text()), 'every chart answered')
    await t.shot(page, 'node-network-charts')
    return
  }

  // From the node page.
  t.check(await t.open(page, `#/node/${HOST}`, /Network/, 20000), 'the node page offers Network')
  await page.getByRole('link', { name: /Network/ }).first().click()
  t.check(await has(/ens1f0np0/, 20000) && /eno1/.test(await text()), 'the grid lists both ports', (await text()).slice(0, 500))
  t.check(/MCP2M00-A002/.test(await text()), 'with the 25G port\'s module')
  await t.shot(page, `node-network-grid-${PHASE}`)
  await page.locator('tr', { hasText: 'ens1f0np0' }).first().click()
  t.check(await t.until(async () => (await cell('data-link')) === 'down'), 'the 25G port: down', await cell('data-link'))
  t.check((await cell('data-fec-active')) === 'baser', 'on baser', await cell('data-fec-active'))
  t.check(/passive copper/.test(await text()), 'its module, said')
  t.check(/rx crc errors/.test(await text()), 'its counters')

  if (PHASE === 'reader') {
    t.check(/admin role/.test(await cell('data-why')), 'a viewer is told why', await cell('data-why'))
    t.check((await page.getByRole('button', { name: 'Change settings' }).count()) === 0, 'and has nothing to press')
    return
  }

  await page.getByRole('button', { name: 'Change settings' }).click()
  await page.locator('input[aria-label=MTU]').fill('big')
  t.check(await has(/MTU: a whole number/), 'a number that is not: said before anything is sent')
  t.check(await page.getByRole('button', { name: 'Save' }).isDisabled(), 'and Save waits')
  await page.locator('input[aria-label=MTU]').fill('')
  await page.locator('select[aria-label=FEC]').selectOption('rs')
  t.check(/fec rs/.test(await cell('data-change')), 'what will be asked', await cell('data-change'))
  t.check(!/does not apply this yet/.test(await text()), 'FEC is applied by the node: no warning')
  await page.getByRole('button', { name: 'Save' }).click()
  t.check(await t.until(async () => /fec "?rs/.test(await cell('data-saved'))), 'saved, as carol', await cell('data-saved'))
  // The node's answer coming back: stormport hands the spec to PID 1, which applies it after two samples.
  t.check(await t.until(async () => (await cell('data-link')) === 'up', 40000), 'the port comes UP', await cell('data-link'))
  t.check((await cell('data-fec-active')) === 'rs', 'on rs', await cell('data-fec-active'))
  t.check(await t.until(async () => /Spec: applied/.test(await cell('data-applied')), 20000), 'Applied', await cell('data-applied'))
  t.check(/baser → rs by networkport\//.test(await cell('data-last-change')), 'the last change, and who asked', await cell('data-last-change'))
  t.check((await cell('data-setting=fec')) === 'rs', 'the setting reads rs', await cell('data-setting=fec'))
  await t.shot(page, 'node-network-applied')

  // A setting the node does not apply yet says so before it is sent.
  await page.getByRole('button', { name: 'Change settings' }).click()
  await page.locator('input[aria-label=MTU]').fill('9000')
  t.check(await has(/MTU: the node does not apply this yet \(stormpump#246\)/), 'MTU: not applied yet, said')
  await page.getByRole('button', { name: 'Cancel' }).click()

  await page.getByRole('button', { name: /All ports/ }).click()
  t.check(await t.until(async () => /\brs\b/.test(await page.locator('tr', { hasText: 'ens1f0np0' }).first().innerText())), 'the grid row reads rs')
})
