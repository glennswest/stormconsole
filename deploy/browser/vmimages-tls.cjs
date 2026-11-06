// The Images pages over the image operator's TLS (#55), from verify-vmimages-tls.sh's real operator.
const { walk } = require('./lib.cjs')
const { WANT } = process.env

walk(`image operator: ${WANT}`, async (t) => {
  const page = await t.page()
  if (WANT === 'read') {
    t.check(await t.open(page, '#/grid?id=img:operator&rel=catalogue', /fedora/i), 'the VM catalogue lists what the operator serves', (await t.text(page)).slice(0, 400))
    await t.shot(page, 'vmimages-catalogue')
    // Create → Virtual machine: its root disks are the operator's.
    await page.getByRole('button', { name: /Create/ }).first().click()
    await page.getByRole('menuitem', { name: /Virtual machine/ }).or(page.getByRole('button', { name: /Virtual machine/ })).first().click()
    const disk = page.getByLabel(/Root disk/)
    t.check(await t.until(async () => (await disk.locator('option').count()) > 0), 'Create VM offers root disks from it')
    t.check(/fedora/i.test((await disk.locator('option').allInnerTexts()).join(' ')), 'by name')
    await t.shot(page, 'vmimages-root-disk')
  } else {
    t.check(await t.open(page, '#/', /answered 403/), 'the overview says the operator refused', (await t.text(page)).slice(0, 400))
    await t.shot(page, 'vmimages-refused')
  }
})
