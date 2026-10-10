// The browser half of deploy/verify-pod-page.sh (#69): a person opening the
// pod list, clicking web-1, and reading every tab — the image and its
// digest, the golden's build, the Service it is behind, live counters
// drawing a rate, and the logs: current and followed, paused, searched,
// the previous run and the runs the console kept. Any page error fails.
const { chromium } = require('playwright')

const BASE = process.env.CONSOLE
const SHOTS = process.env.SHOTS
let failed = 0
const errors = []
function check(ok, what, extra = '') {
  console.log(`  ${ok ? 'ok  ' : 'FAIL'} ${what}${extra ? ` — ${extra}` : ''}`)
  if (!ok) failed++
}
const pause = (ms) => new Promise((r) => setTimeout(r, ms))

;(async () => {
  const browser = await chromium.launch()
  const page = await browser.newPage({ viewport: { width: 1440, height: 1000 } })
  page.on('pageerror', (e) => errors.push(`pageerror: ${e.message.split('\n')[0]}`))
  page.on('console', (m) => {
    if (m.type() === 'error') errors.push(`console: ${m.text().split('\n')[0]}`)
  })
  const shot = (n) => page.screenshot({ path: `${SHOTS}/${n}.png`, fullPage: true })
  const tab = (t) => page.locator('nav.tabs button', { hasText: t }).click()

  // The list, then the row: clicking a pod opens its page.
  await page.goto(`${BASE}/#/k8s/pod?ns=shop`)
  await page.getByText('web-1').first().waitFor({ timeout: 15000 })
  await page.getByText('web-1').first().click()
  await page.waitForFunction(() => location.hash.startsWith('#/pod/shop/web-1'), null, { timeout: 5000 }).catch(() => {})
  check((await page.evaluate(() => location.hash)).startsWith('#/pod/shop/web-1'), 'clicking the row opens the pod page', await page.evaluate(() => location.hash))
  if (!(await page.evaluate(() => location.hash)).startsWith('#/pod/')) await page.goto(`${BASE}/#/pod/shop/web-1`)

  // Overview
  await page.locator('h2', { hasText: 'Containers' }).waitFor({ timeout: 15000 })
  const body = async () => page.locator('.sc-page').innerText()
  let text = await body()
  check(text.includes('ReplicaSet web-7d9f') && text.includes('Deployment web'), 'owned by ReplicaSet ← Deployment')
  check(/sha256:[0-9a-f]{64}/.test(text), 'a digest is shown')
  check(text.includes('cilium@abc123def456') && text.includes('by stormcentral'), "the stormpump image's registry image and who built it")
  check(text.includes('rustkube-node#130'), 'what the node does not report names its issue')
  check(text.includes('migrate') && text.includes('init'), 'the init container is listed')
  check(text.includes('Burstable'), 'QoS')
  await pause(6000)
  text = await body()
  check(/↓ [\d.]+ ?[KMG]?i?B\/s/.test(text), 'Overview traffic shows a receive rate', (text.match(/↓ [^\n]*/) || [''])[0])
  await shot('1-overview')

  // Probes, volumes, requests and limits (#124).
  check(/liveness\s+http-get http:\/\/:8080\/healthz/.test(text), 'the liveness probe as configured', (text.match(/liveness[^\n]*/) || [''])[0])
  check(/delay=0s timeout=1s period=5s #success=1 #failure=3/.test(text), 'its timing, defaults filled in')
  const agentLive = await page.locator('tr', { hasText: 'cat /tmp/healthy' }).innerText()
  check(/failing/.test(agentLive) && /No such file/.test(agentLive) && /×7/.test(agentLive), "agent's liveness failing, its last failure ×7", agentLive.replace(/\s+/g, ' '))
  const appReady = await page.locator('tr', { hasText: 'tcp-socket :http' }).innerText()
  check(/passing/.test(appReady) && /×4/.test(appReady), "app's readiness passing, its last failure kept", appReady.replace(/\s+/g, ' '))
  const data = await page.locator('tr', { hasText: '/var/lib/web' }).innerText()
  check(/web-data/.test(data) && /Bound/.test(data) && /10Gi/.test(data) && /stormblock\.storm\.io/.test(data), 'the data volume: claim Bound, 10Gi, its PV', data.replace(/\s+/g, ' '))
  check(/cpu 100m, memory 64Mi/.test(text) && /memory 256Mi/.test(text), 'requests and limits per container')
  check(/✓ ready/.test(text), 'ready per container')

  // Network
  await tab('Network')
  await page.locator('h2', { hasText: 'Services that select this pod' }).waitFor()
  await pause(6000)
  text = await body()
  check(text.includes('receiving traffic'), 'the Service lists this pod as receiving traffic')
  check(text.includes('fd00:10:244::f'), 'both addresses')
  check(text.includes('shop.svc.cluster.local'), 'DNS search')
  const points = await page.locator('svg.spark polyline').first().getAttribute('points')
  check(!!points && points.split(' ').length >= 2, 'the receive sparkline is drawn', points || 'none')
  check(text.includes('rustkube-node#131'), 'interface detail and missing counters name rustkube-node#131')
  await shot('2-network')

  // Logs: current, followed
  await tab('Logs')
  const out = page.locator('pre.out')
  await out.waitFor()
  await page.waitForFunction(() => document.querySelector('pre.out')?.innerText.includes('live'), null, { timeout: 10000 }).catch(() => {})
  let log = await out.innerText()
  check(log.includes('app run 4 line 999'), 'opens on the latest lines (tail)', log.split('\n').slice(-2).join(' | '))
  check(log.includes('live'), 'following appends as the container writes')
  check((await page.locator('.live').count()) > 0, 'says it is live')

  // Pause holds new lines back and says how many.
  await page.getByRole('button', { name: 'Pause' }).click()
  const before = (await out.innerText()).split('\n').length
  await pause(2000)
  const held = (await out.innerText()).split('\n').length
  check(held === before, 'paused: nothing is added', `${before} → ${held}`)
  const resume = page.getByRole('button', { name: /Resume/ })
  check(/\(\d+ new\)/.test(await resume.innerText()), 'Resume says how many are waiting', await resume.innerText())
  await resume.click()
  await pause(300)
  check((await out.innerText()).split('\n').length > held, 'resume adds them')

  // Search highlights.
  await page.getByLabel('Search the log').fill('line 99')
  await pause(300)
  check((await page.locator('pre.out mark').count()) > 0, 'search marks matches')
  await shot('3-logs-current')
  await page.getByLabel('Search the log').fill('')

  // Download link.
  const href = await page.getByRole('link', { name: 'Download' }).getAttribute('href')
  check(href.includes('download=true') && !href.includes('follow'), 'Download is the whole log, not the stream', href)

  // Previous, from the node.
  await page.getByLabel('Which run').selectOption('previous')
  await page.waitForFunction(() => document.querySelector('pre.out')?.innerText.includes('panic'), null, { timeout: 8000 }).catch(() => {})
  log = await out.innerText()
  check(log.includes('app run 3: panic'), 'Previous is the run before', log.split('\n').slice(-1)[0])

  // The runs the console kept.
  const options = await page.getByLabel('Which run').locator('option').allInnerTexts()
  check(options.some((o) => o.startsWith('run 0')) && options.some((o) => o.startsWith('run 3') && o.includes('2 missed')), 'kept runs listed, with what was missed', options.join(' / '))
  await page.getByLabel('Which run').selectOption('run:0')
  await page.waitForFunction(() => document.querySelector('pre.out')?.innerText.includes('run 0: panic'), null, { timeout: 8000 }).catch(() => {})
  check((await out.innerText()).includes('app run 0: panic'), 'a kept run opens')
  await shot('4-logs-kept')

  // Another container.
  await page.getByLabel('Container').selectOption('agent')
  await page.waitForFunction(() => document.querySelector('pre.out')?.innerText.includes('agent run'), null, { timeout: 8000 }).catch(() => {})
  check((await out.innerText()).includes('agent run'), 'the stormpump container has logs too')

  // Stats (#124): CPU, memory and network over the window.
  await tab('Stats')
  await page.locator('h2', { hasText: 'CPU' }).waitFor({ timeout: 15000 })
  await page.waitForFunction(() => document.querySelectorAll('.stats svg path').length >= 3, null, { timeout: 40000 }).catch(() => {})
  const paths = await page.locator('.stats svg path').count()
  check(paths >= 3, 'CPU, memory and network are drawn', `${paths} lines`)
  text = await body()
  check(/app\s+\d+m/.test(text) && /agent/.test(text), 'CPU legend: each container with its current value', (text.match(/CPU[\s\S]{0,80}/) || [''])[0].replace(/\s+/g, ' '))
  check(/48\.0 MB/.test(text), "app's working set")
  check(/received/.test(text) && /sent/.test(text), 'network received and sent')
  check(/rustkube-node#242/.test(text), 'what the node does not report names rustkube-node#242')
  const svg = page.locator('.stats svg').first()
  const box = await svg.boundingBox()
  await page.mouse.move(box.x + box.width - 20, box.y + box.height / 2)
  await pause(300)
  check((await page.locator('.stats .tip').count()) > 0, 'hovering a chart shows the values at that time')
  await shot('6-stats')
  await page.getByLabel('Window').selectOption('3600')
  await pause(1500)
  check((await page.locator('.stats svg path').count()) >= 3, 'the hour window draws too')

  // Events (#124): the table, followed.
  await tab('Events')
  await page.locator('table.events').waitFor({ timeout: 10000 })
  const first = await page.locator('table.events tbody tr').first().innerText()
  check(/Unhealthy/.test(first) && /agent/.test(first) && /\b7\b/.test(first) && /kubelet on n1/.test(first), 'newest first: Unhealthy, its container, count, source', first.replace(/\s+/g, ' '))
  check((await page.locator('table.events tbody tr').count()) === 3, 'every event about this pod')
  await shot('7-events')
  await tab('YAML')
  await page.waitForFunction(() => document.querySelector('.sc-page')?.innerText.includes('web-7d9f'), null, { timeout: 8000 }).catch(() => {})
  check((await body()).includes('kind: Pod') || (await body()).includes('web-7d9f'), 'YAML shows the object')
  await shot('5-yaml')

  // The row's Logs action lands on the Logs tab.
  await page.goto(`${BASE}/#/pod/shop/web-1?tab=Logs`)
  await page.locator('pre.out').waitFor({ timeout: 10000 })
  check(true, '?tab=Logs opens on the logs')

  for (const e of errors) console.log(`  ${e}`)
  check(errors.length === 0, 'no page errors', `${errors.length}`)
  await browser.close()
  console.log(failed ? `browser: ${failed} failed` : 'browser: all ok')
  process.exit(failed ? 1 : 0)
})().catch((e) => {
  console.log(`FAIL ${e.message}`)
  process.exit(1)
})
