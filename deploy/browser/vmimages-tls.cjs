// The Images pages over the image operator's TLS (#55), from verify-vmimages-tls.sh's real operator.
const { walk } = require('./lib.cjs')
const { WANT } = process.env

walk(`image operator: ${WANT}`, async (t) => {
  const page = await t.page()
  if (WANT === 'read') {
    t.check(await t.open(page, '#/grid?id=img:operator&rel=catalogue', /fedora/i), 'the VM catalogue lists what the operator serves', (await t.text(page)).slice(0, 400))
    await t.shot(page, 'vmimages-catalogue')
  } else {
    t.check(await t.open(page, '#/', /answered 403/), 'the overview says the operator refused', (await t.text(page)).slice(0, 400))
    await t.shot(page, 'vmimages-refused')
  }
})
