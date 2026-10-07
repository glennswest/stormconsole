// The VM pages in a browser (#58, for #24, #25, #26, #37), one MODE per rig:
//   net        deploy/verify-vm-net.sh — the Network card and the list's addresses
//   keys       deploy/verify-vm-keys.sh — Account → SSH keys and a machine's keys card
//   snapshots  deploy/verify-vm-snapshots.sh — the Backup tab
//   lifecycle  deploy/verify-vm-lifecycle.sh — the row's Start / Restart / Stop
const { walk } = require('./lib.cjs')
const MODE = process.env.MODE

const tab = (page, name) => page.locator('nav.tabs button', { hasText: name }).click()

walk(`vm: ${MODE}`, async (t) => {
  if (MODE === 'net') {
    const page = await t.page()
    const net = () => page.locator('.card', { hasText: 'Asked for' }).innerText()
    t.check(await t.open(page, '#/vm/default/nat', /Network/), 'nat: the page opens')
    t.check(await t.until(async () => /NAT inside the hypervisor/.test(await net())), 'nat: the node did NAT', await net().catch(() => ''))
    const n = await net()
    t.check(/10\.155\.0\.15/.test(n) && /stormvm#16/.test(n), 'nat: its address, and why nothing outside reaches it', n)
    t.check(/pod network \(masquerade\)/.test(n), 'nat: what was asked for', n)
    await t.shot(page, 'vm-net-nat')
    t.check(await t.open(page, '#/vm/default/bridged', /192\.168\.8\.61/), 'bridged: its v4 address')
    t.check(/fd00:8::61/.test(await t.text(page)) && /host bridge stormbr0/.test(await t.text(page)), 'bridged: its v6 address and the bridge')
    t.check(await t.open(page, '#/vm/default/quiet', /no address yet/), 'quiet: no address yet')
    t.check(await t.open(page, '#/vms', /10\.155\.0\.15/), 'the list carries the addresses')
    t.check(/NAT, not pod/.test(await t.text(page)), 'and says NAT, not pod')
    await t.shot(page, 'vm-list')
  }

  if (MODE === 'keys') {
    const page = await t.page('gw')
    t.check(await t.open(page, '#/account/keys', /Saved keys/), 'Account → SSH keys opens')
    const saved = () => page.locator('.card', { hasText: 'Saved keys' }).innerText()
    t.check(/laptop/.test(await saved()), 'the saved keys are listed', await saved())
    t.check(/From the console's configuration/.test(await t.text(page)), "and the configuration's key, apart")
    await page.locator('input[aria-label="Key name"]').fill('browser')
    await page.locator('textarea[aria-label="Public key"]').fill(process.env.KEY)
    await page.getByRole('button', { name: 'Save', exact: true }).click()
    t.check(await t.until(async () => /browser/.test(await saved())), 'Save: the pasted key is listed', await saved())
    await page.locator('.card', { hasText: 'Saved keys' }).locator('tr', { hasText: 'browser' }).getByRole('button', { name: 'Delete' }).click()
    t.check(await t.until(async () => !/\bbrowser\b/.test(await saved())), 'Delete: it is gone')
    await t.shot(page, 'account-keys')

    t.check(await t.open(page, '#/vm/web/web-1', /SSH keys/), "web-1's page has its keys card")
    const card = () => page.locator('.card', { hasText: 'SSH keys' }).first().innerText()
    t.check(await t.until(async () => /laptop/.test(await card())), 'listing the keys it was made with', await card())
    t.check(await t.open(page, '#/vm/web/old', /SSH keys/), 'old (made elsewhere) opens')
    const add = page.getByRole('button', { name: /Add my keys|Refresh my keys/ })
    if (await add.count()) {
      await add.click()
      t.check(await t.until(async () => /laptop/.test(await card())), 'Add my keys: they are on it', await card())
    } else {
      t.check(/laptop/.test(await card()), 'old already carries the keys', await card())
    }
    const reader = await t.page('reader')
    t.check(await t.open(reader, '#/account/keys', /SSH keys|Saved keys|No keys/i), 'reader opens the page')
    t.check((await reader.getByRole('button', { name: 'Save', exact: true }).count()) === 0, 'and is offered no Save')
  }

  if (MODE === 'snapshots') {
    const page = await t.page()
    t.check(await t.open(page, '#/vm/default/web-1', /web-1/), 'web-1 opens')
    await tab(page, 'Backup')
    const main = () => t.text(page)
    t.check(await t.until(async () => /pre-upgrade/.test(await main())), 'Backup: pre-upgrade is listed', (await main()).slice(0, 400))
    const pre = await page.locator('tr', { hasText: 'pre-upgrade' }).first().innerText()
    t.check(/before 10\.1/.test(pre) && /root/.test(pre) && /data/.test(pre), 'with its note and its disks', pre)
    t.check(/freezing/.test(await main()) && /guest agent did not answer/.test(await main()), 'the failed one says the step and why', (await main()).slice(0, 600))
    await page.getByRole('button', { name: 'Snapshot', exact: true }).click()
    await page.locator('input[aria-label="Snapshot name"]').fill('from-the-browser')
    await page.locator('input[aria-label="Note"]').fill('taken in Chromium')
    await page.getByRole('button', { name: 'Take snapshot' }).click()
    t.check(await t.until(async () => /from-the-browser/.test(await main())), 'Take snapshot: it is listed', (await main()).slice(0, 400))
    await t.shot(page, 'vm-backup')
  }

  if (MODE === 'lifecycle') {
    const page = await t.page()
    const row = (id) => page.locator('tbody tr', { hasText: id }).first()
    t.check(await t.open(page, '#/vms', /web-1/), 'the list opens')
    const r = await row('web-1').innerText()
    t.check(/Failed/i.test(r) && /could not open disk root/.test(r), 'web-1: Failed, with the reason on the row', r)
    const restart = row('web-1').getByRole('button', { name: 'Restart', exact: true })
    t.check(await restart.isEnabled(), 'Restart is offered on a failed machine')
    await restart.click()
    t.check(await t.until(async () => !/Failed/i.test(await row('web-1').innerText()), 20000), 'Restart: the dead instance goes', await row('web-1').innerText())
    const b = await row('bare-1').innerText()
    t.check(/Failed/i.test(b), 'bare-1: Failed', b)
    t.check(await row('bare-1').getByRole('button', { name: 'Restart', exact: true }).isDisabled(), 'and nothing to restart it from')
    await t.shot(page, 'vm-lifecycle')
  }
})
