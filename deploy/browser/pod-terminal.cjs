// A pod's Terminal and Environment (#12), over deploy/verify-pod-terminal.sh. PHASE: alice, bob, carol, dave.
const { walk } = require('./lib.cjs')
const PHASE = process.env.PHASE

walk(`pod terminal (${PHASE})`, async (t) => {
  const page = await t.page(PHASE)
  const tab = (n) => page.locator('nav.tabs button', { hasText: n }).click()
  t.check(await t.open(page, '#/pod/shop/web', /Overview/), 'the pod page')
  await tab('Terminal')
  const connect = page.getByRole('button', { name: /^Connect/ })
  const screen = () => page.locator('.xterm-rows').innerText().catch(() => '')
  const seen = (re, ms = 10000) => t.until(async () => re.test((await screen()).replace(/ /g, ' ')), ms)
  const type = async (s) => {
    await page.locator('.xterm-helper-textarea').focus()
    await page.keyboard.type(s)
    await page.keyboard.press('Enter')
  }

  if (PHASE === 'carol') {
    t.check(await t.until(async () => /needs the operator role/.test(await t.text(page))), 'a reader is told why', (await t.text(page)).slice(0, 300))
    t.check((await connect.count()) === 0, 'and has nothing to press')
    return
  }
  if (PHASE === 'dave') {
    t.check(await t.until(async () => /no kubernetes identity/.test(await t.text(page))), 'no identity: told why', (await t.text(page)).slice(0, 300))
    t.check((await connect.count()) === 0, 'and has nothing to press')
    return
  }
  if (PHASE === 'bob') {
    await connect.click()
    t.check(await t.until(async () => /not allowed to create pods\/exec/.test(await t.text(page))), 'view, not edit: the apiserver says no, in its words', await page.locator('p.result').innerText().catch(() => 'no result line'))
    await tab('Environment')
    t.check(await t.until(async () => /could not be read/.test(await t.text(page))), 'a Secret he may not read is said to be', (await t.text(page)).slice(0, 500))
    t.check(/from-configmap/.test(await t.text(page)), 'a ConfigMap he may read has its value')
    return
  }

  // alice
  await connect.click()
  const ok = await seen(/\$ /)
  t.check(ok, 'a prompt', `${await screen()} | ${await page.locator('p.result').innerText().catch(() => 'no result line')}`)
  if (!ok) return
  await type('echo hello-$((40+2)) in $CONTAINER')
  t.check(await seen(/hello-42 in app/), 'a real shell, in the first container', await screen())
  await t.shot(page, 'terminal')
  await type('stty size')
  t.check(await seen(/\b\d+ \d+\b/), 'the pty has a size', await screen())
  const before = (await screen()).replace(/ /g, ' ').match(/\b(\d+) (\d+)\b/)[0]
  await page.setViewportSize({ width: 1000, height: 1000 })
  await t.pause(800)
  await type('clear; stty size')
  t.check(await t.until(async () => {
    const m = (await screen()).replace(/ /g, ' ').match(/\b(\d+) (\d+)\b/)
    return m && m[0] !== before
  }), 'a smaller window resizes it', `${before} → ${await screen()}`)
  await page.setViewportSize({ width: 1440, height: 1000 })
  await type('C=31; printf "\\033[${C}mred\\033[0m\\n"')
  t.check(await seen(/\bred\b/), 'escape codes are drawn, not printed')
  t.check(!/\[31m/.test(await screen()), 'and not shown as text')
  await type('exit 3')
  t.check(await t.until(async () => /exit code 3/.test(await t.text(page))), 'the result says its exit code', (await t.text(page)).slice(-300))

  await page.locator('select[aria-label=Container]').selectOption('sidecar')
  await page.getByRole('button', { name: /Connect again/ }).click()
  t.check(await seen(/\$ /), 'connects again')
  await type('echo in $CONTAINER')
  t.check(await seen(/in sidecar/), 'the chosen container', await screen())
  await page.getByRole('button', { name: 'Disconnect' }).click()
  t.check(await t.until(() => connect.isVisible()), 'disconnects')

  await page.locator('input[aria-label=Command]').fill('nosuchcommand --flag')
  await connect.click()
  t.check(await t.until(async () => /nosuchcommand.*not found/.test(await t.text(page))), 'an image with no such command: the runtime’s words', (await t.text(page)).slice(-300))

  await tab('Environment')
  const env = page.locator('table.env')
  t.check(await t.until(async () => /NODE/.test(await env.innerText())), 'the Environment tab', await t.text(page))
  const text = await env.innerText()
  t.check(/PLAIN\s+literal/.test(text), 'a literal')
  t.check(/NODE\s+n1\s+field spec\.nodeName/.test(text), 'a field, read off the pod', text)
  t.check(/URL\s+from-configmap\s+ConfigMap conf key url/.test(text), 'a ConfigMap value, as the viewer')
  t.check(/C_url\s+from-configmap/.test(text), 'envFrom, with its prefix')
  t.check(/PASSWORD\s+value not shown\s+Secret db key password/.test(text), 'a Secret: where from, and not what', text)
  t.check(/user\s+value not shown/.test(text), 'an envFrom Secret: its keys')
  t.check(/JOINED\s+n1-\$\(PASSWORD\)/.test(text), 'expanded, and no Secret into it')
  const html = await page.content()
  t.check(!/hunter2|aHVudGVyMg/.test(html), 'the value is nowhere in the page')
  await t.shot(page, 'environment')
})
