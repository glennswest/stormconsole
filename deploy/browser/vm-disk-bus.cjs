// The disk buses a person is offered (#38), over deploy/verify-vm-disk-bus.sh.
const { walk } = require('./lib.cjs')

const BUSES = ['virtio', 'nvme', 'scsi', 'sata']

walk('vm disk bus', async (t) => {
  const page = await t.page()
  const dialog = page.locator('[role=dialog]')
  const field = (label) => dialog.locator('label', { has: page.locator('.lbl', { hasText: new RegExp(`^${label}\\*?$`) }) })

  t.check(await t.open(page, '#/vms', /Virtual machines/), 'the machines page')
  await page.getByRole('button', { name: /Create/ }).first().click()
  await page.getByRole('menuitem').filter({ hasText: /^Virtual machine/ }).first().click()
  const created = await field('Disk bus').locator('select option').allInnerTexts()
  t.check(created.join() === BUSES.join(), 'Create offers the four buses', created.join())
  await page.keyboard.press('Escape')
  if (await dialog.isVisible()) await dialog.getByRole('button', { name: /Cancel/ }).click()

  t.check(await t.open(page, '#/vm/lab/m-virtio', /m-virtio/), 'a machine opens')
  await page.getByRole('button', { name: 'Add disk' }).click()
  const bus = page.locator('select[aria-label="Disk bus"]')
  const offered = await bus.locator('option').allInnerTexts()
  t.check(offered.join() === BUSES.join(), 'Add disk offers the four buses', offered.join())
  await page.locator('select[aria-label="Disk source"]').selectOption('empty')
  await page.locator('input[aria-label="Disk source name"]').fill('1Gi')
  await page.locator('input[aria-label="Disk name"]').fill('walked')
  await bus.selectOption('nvme')
  await t.shot(page, 'add-disk-nvme')
  await page.getByRole('button', { name: /^Add$/ }).click()
  t.check(await t.until(async () => /walked/.test(await t.text(page)) && !(await bus.isVisible())), 'the disk is added', await t.text(page))

  await page.locator('nav.tabs button', { hasText: 'Settings' }).click()
  const row = page.locator('table.settings tr', { has: page.locator('th', { hasText: /^Disk bus$/ }) })
  await row.locator('button', { hasText: 'Change' }).click()
  await row.locator('input').fill('usb')
  await row.locator('button', { hasText: 'Save' }).click()
  t.check(await t.until(async () => /not a disk bus this console offers/.test(await t.text(page))), 'Settings refuses usb and names the buses', await t.text(page))
})
