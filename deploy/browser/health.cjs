// API health in a browser (#123), over deploy/verify-api-health.sh's real
// stormd and stand-in service: the alert bar on every page while an API is
// stalled, the #/health page and a picked API's changes; after recovery,
// no bar, and a console whose PID 1 summary holds a stalled engine probe
// still shows its alert.
const { walk } = require('./lib.cjs')
const PHASE = process.env.PHASE
const STALE = process.env.STALE

walk(`health (${PHASE})`, async (t) => {
  const page = await t.page()
  const bar = page.locator('.alertbar')

  if (PHASE === 'stalled') {
    // Any page carries it: the overview first.
    await page.goto(`${t.BASE}/#/`)
    t.check(await t.until(async () => (await bar.count()) === 1), 'the alert bar is on the overview')
    const said = await bar.innerText()
    t.check(/standin API things STALLED for \d+s — no answer within 2 s/.test(said), 'it names the service, the probe, how long and why', said)
    await t.shot(page, 'health-alert-overview')

    // And another page.
    await page.goto(`${t.BASE}/#/nodes`)
    t.check(await t.until(async () => (await bar.count()) === 1), 'and on the nodes page')

    // The bar leads to the API.
    await bar.locator('.body a').first().click()
    t.check(await t.until(async () => /#\/health\?api=/.test(page.url())), 'clicking it opens the API on #/health', page.url())
    t.check(await t.until(async () => /Changes of standin API things/i.test(await t.text(page))), 'with its changes', (await t.text(page)).slice(0, 400))

    t.check(await t.open(page, '#/health', /standin/), 'the API health page lists the API', (await t.text(page)).slice(0, 300))
    const text = await t.text(page)
    t.check(/From each stormd on this node \(:19580\)/.test(text), 'it says it read each stormd')
    t.check(/storage engine/.test(text) && /stormcos#525/.test(text), 'and what that misses')
    t.check(/1 stalled/i.test(text), 'the count band says one stalled')
    const row = page.locator('table tbody tr', { hasText: 'standin' }).first()
    const r = await row.innerText()
    t.check(/stalled/.test(r) && /things/.test(r) && /no answer within 2 s/.test(r) && /– \/ 200 ms/.test(r), 'the row: state, API, error, budget', r)
    await row.getByRole('button').click()
    t.check(await t.until(async () => /GET http:\/\/127\.0\.0\.1:19501\/api\/v1\/things/.test(await t.text(page))), 'a picked API shows its probe')
    t.check(/every 1s/.test(await t.text(page)), 'and its interval')
    await t.shot(page, 'health-page-stalled')
    await page.locator('input[aria-label="Search"]').fill('nothing-like-this')
    t.check(await t.until(async () => /Nothing matches the search/.test(await t.text(page))), 'search filters')
  } else {
    await page.goto(`${t.BASE}/#/`)
    await t.pause(3000)
    t.check((await bar.count()) === 0, 'no alert bar once the API is healthy')
    t.check(await t.open(page, '#/health', /healthy/), 'the page says healthy')
    const text = await t.text(page)
    t.check(/1 API, healthy/.test(text), 'the node sentence', text.slice(0, 300))
    // The kept changes the rig wrote.
    t.check(/Changes on this node/i.test(text) && /healthy →/.test(text) && /no answer within 2 s/.test(text), 'every kept change on the node', text.slice(-600))
    await t.shot(page, 'health-page-healthy')

    // The console reading PID 1's summary: the engine's own probe.
    const other = await t.page()
    await other.goto(`${STALE}/#/health`)
    const obar = other.locator('.alertbar')
    t.check(await t.until(async () => (await obar.count()) === 1), 'from the summary: the bar is up')
    const said = await obar.innerText()
    t.check(/00-stormblock API volumes STALLED for .* — no answer within 10 s/.test(said), 'for PID 1\'s own probe of the engine', said)
    const otext = await other.locator('main').innerText()
    t.check(/has not been rewritten/.test(otext), 'and the page says the summary stopped')
    t.check(/no history at/.test(otext) && /not mounted into the console/.test(otext), 'and that no history is mounted')
    await other.screenshot({ path: `${process.env.SHOTS}/health-summary.png`, fullPage: true }).catch(() => {})
  }
})
