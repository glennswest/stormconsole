// Images, Volumes and Unattached in a browser (#58, for #19), over
// deploy/verify-images.sh's real stormblock and sbregistry.
const { walk } = require('./lib.cjs')

walk('images, volumes, unattached', async (t) => {
  const page = await t.page()
  const main = () => t.text(page)

  t.check(await t.open(page, '#/images', /pvc-256M/, 20000), 'Images: the catalog lists pvc-256M', (await main()).slice(0, 300))
  const cat = await main()
  t.check(/media\/missing/.test(cat) && /failed/i.test(cat), 'the failed fetch is a row, and says so', cat.slice(0, 600))
  t.check(/arriving or failed/.test(cat), 'and the page counts it')
  const row = page.locator('tr', { hasText: 'pvc-256M' }).first()
  t.check(/\b2\b/.test(await row.innerText()), 'pvc-256M: 2 clones', await row.innerText())
  await page.locator('input[aria-label="Search images"]').fill('pvc-256')
  t.check(await t.until(async () => !/media\/missing/.test(await main())), 'search narrows it')
  await t.shot(page, 'images')

  t.check(await t.open(page, '#/grid?id=sb:engine&rel=volumes', /shop\/db/, 20000), 'Volumes: the claim and its consumer', (await main()).slice(0, 300))
  t.check(!/idle-clone/.test(await main()), 'and not the clone nothing uses')
  await t.shot(page, 'volumes')

  t.check(await t.open(page, '#/grid?id=sb:engine&rel=unattached', /idle-clone/, 20000), 'Unattached: the idle clone', (await main()).slice(0, 300))
  t.check(/\bseed\b/.test(await main()), 'and the seed')
  t.check(!/shop\/db/.test(await main()), 'not the attached claim')
  await t.shot(page, 'unattached')
})
