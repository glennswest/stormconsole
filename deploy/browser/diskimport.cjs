// Importing a disk and using it, in a browser (#44), over deploy/verify-disk-import.sh.
const { walk } = require('./lib.cjs')
const { G2, U } = process.env

walk('disk import', async (t) => {
  const page = await t.page('root')
  const dialog = page.locator('[role=dialog]')
  const field = (label) => dialog.locator('label', { has: page.locator('.lbl', { hasText: new RegExp(`^${label}\\*?$`) }) })
  const openCreate = async (item) => {
    await page.getByRole('button', { name: /Create/ }).first().click()
    await page.getByRole('menuitem').filter({ hasText: item }).first().click()
  }

  t.check(await t.open(page, '#/images', /Catalog|Images/), 'the catalog opens')
  await openCreate(/Import a disk/)
  await field('URL').locator('input').fill(`${U}/nothere-either.img`)
  await field('Name').locator('input').fill('Bad Name')
  await t.shot(page, 'import-form')
  await dialog.getByRole('button', { name: /^Create|Import/ }).last().click()
  t.check(await t.until(async () => /not a usable name/.test(await dialog.innerText())), 'a bad name is said on the form', await dialog.innerText())
  await field('Name').locator('input').fill('walked')
  await field('URL').locator('input').fill(`${U}/guest.qcow2`)
  await dialog.getByRole('button', { name: /^Create|Import/ }).last().click()
  t.check(await t.until(async () => !(await dialog.isVisible())), 'accepted, the form closes')
  t.check(await t.until(async () => /importing walked|Catalog|media/.test(await t.text(page))), 'it says so')

  t.check(await t.open(page, '#/vms', /Virtual machines/), 'the machines page')
  await openCreate(/^Virtual machine/)
  const root = field('Root disk').locator('select')
  t.check((await root.locator('option').allInnerTexts()).some((o) => /\(imported\)/.test(o)), 'the root disk list offers it')
  await root.selectOption(G2)
  await dialog.locator('select[aria-label=Project]').selectOption('shop')
  await field('Name').locator('input').fill('imported')
  await t.shot(page, 'import-vm')
  await dialog.getByRole('button', { name: /^Create Virtual machine$/ }).click()
  t.check(await t.until(async () => !(await dialog.isVisible()), 15000), 'the machine is created', await dialog.innerText().catch(() => ''))
})
