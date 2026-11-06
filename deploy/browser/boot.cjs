// The Boot and flow-over page (#271), over verify-history.sh. FLOW is the stand-in engine, or empty for a real one with no flow-over.
const { walk } = require('./lib.cjs')
const { FLOW, PHASE } = process.env

const mode = (m) => fetch(`${FLOW}/_mode/${m}`, { method: 'POST' })

walk(`boot (${PHASE})`, async (t) => {
  const page = await t.page('')
  await page.setViewportSize({ width: 1920, height: 1080 })
  page.upstreamDown = PHASE === 'gone'
  const word = () => page.locator('[data-testid=flow-word]').innerText()
  const says = (re) => t.until(async () => re.test(await word()), 20000)
  await page.goto(`${t.BASE}/#/boot`)

  const boot = page.locator('[data-testid=boot-card]')
  if (PHASE === 'bare') {
    t.check(await t.until(async () => /diskless|no system-data|not mounted|keeps no record/i.test(await boot.innerText())), 'a node with no system-data says so', await boot.innerText().catch(() => ''))
    return
  }
  t.check(await t.until(async () => /modules/.test(await boot.innerText())), "the Boot card has PID 1's phases", await boot.innerText().catch(() => ''))
  if (PHASE === 'none') {
    t.check(/firmware end to PID 1/.test(await boot.innerText()), 'and what came before PID 1')
    t.check(await says(/./), 'the real engine answers', await word())
    t.check(/own disk|No flow-over/.test(await word()), 'a real engine with no flow-over says so', await word())
    t.check((await page.locator('[role=progressbar]').count()) === 0, 'and draws no progress')
    await t.shot(page, 'boot-real-engine')
    return
  }
  if (PHASE === 'gone') {
    t.check(await says(/cannot be asked/), 'no engine: said', await word())
    return
  }

  t.check(/n1|node|\w/.test(await page.locator('[data-testid=host]').innerText()), 'the machine is named')
  t.check(await page.locator('.shell').count() === 0 && await page.locator('nav').count() === 0, 'full page: no masthead or navigator')
  const c0 = await page.locator('[data-testid=clock] .digits').innerText()
  await t.pause(2500)
  const c1 = await page.locator('[data-testid=clock] .digits').innerText()
  t.check(c0 !== c1, 'the clock runs', `${c0} ${c1}`)
  t.check(/\d/.test(await page.locator('[data-testid=all-running] .digits').innerText()), 'how long to every service running', await boot.innerText())

  await mode('moving')
  t.check(await says(/Moving onto its own disk/), 'moving', await word())
  const percent = page.locator('[data-testid=flow-percent]')
  const p0 = parseInt(await percent.innerText())
  t.check(await t.until(async () => (await page.locator('[data-testid=flow-spark] polyline').count()) === 1, 20000), 'a sparkline, once there are readings')
  t.check(await t.until(async () => parseInt(await percent.innerText()) > p0, 30000), 'the percent rises, live', `${p0} ${await percent.innerText()}`)
  const sp = parseFloat(await page.locator('[data-testid=flow-speed]').innerText())
  t.check(sp > 100 && sp < 500, 'MB/s from the byte counter, within what the engine moves', String(sp))
  const flat = await page.locator('[data-testid=flow-spark] polyline').getAttribute('points')
  t.check(new Set(flat.split(' ').map((p) => p.split(',')[1])).size > 2, 'and it has a shape, not an average', flat)
  const fits = await page.evaluate(() => { const b = document.querySelector('.boot'); return b.scrollHeight <= b.clientHeight && b.scrollWidth <= b.clientWidth })
  t.check(fits, 'it fits 1920x1080 with no scrolling')
  await t.shot(page, 'boot-moving')

  for (const [m, re, sub] of [
    ['checking', /Checking what moved/, /read back/],
    ['stalled', /stopped/, /destination full/],
    ['corrupt', /did not read back/, /does not go local/],
    ['settling', /settling/, /network/],
  ]) {
    await mode(m)
    t.check(await says(re), m, await word())
    t.check(!/own disk/.test(await word()), 'and does not claim its own disk')
    t.check(sub.test(await page.locator('[data-testid=flow-sub]').innerText()), 'and says why')
    await t.shot(page, `boot-${m}`)
  }
  await mode('done')
  t.check(await says(/Running from its own disk/), 'at the end: running from its own disk', await word())
  t.check((await page.locator('[role=progressbar]').count()) === 0, 'the bar has gone')
  t.check(/MB\/s on average/.test(await page.locator('[data-testid=flow-total]').innerText()), 'what it moved, and how fast')
  await t.shot(page, 'boot-done')

  // A phone is not what this is for, and it still has to read.
  await page.setViewportSize({ width: 390, height: 800 })
  t.check(await page.evaluate(() => { const b = document.querySelector('.boot'); return b.scrollWidth <= b.clientWidth }), 'no sideways scrolling on a phone')
  await t.shot(page, 'boot-phone')
})
