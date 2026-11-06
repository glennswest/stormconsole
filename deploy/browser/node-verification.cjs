// Each node's last verification (#74): the failed one stands out on Nodes, and its page says what failed.
const { walk } = require('./lib.cjs')
const { HOST } = process.env

walk('node verification', async (t) => {
  const page = await t.page()
  t.check(await t.open(page, '#/nodes', /FAILED, 2 problems/), 'Nodes shows the failed verification', (await t.text(page)).slice(0, 500))
  const text = await t.text(page)
  t.check(/passed, /.test(text), 'and a passed one')
  t.check(/first pass running/.test(text), 'and one before its first pass')
  t.check(/need/.test(text) && /attention/.test(text), 'the header counts what needs attention')
  await t.shot(page, 'verification-nodes')

  await page.getByLabel('Filter by state').selectOption('failed')
  await t.pause(300)
  const failed = await t.text(page)
  t.check(/storm-b/.test(failed), 'the filter keeps the failed node')
  t.check(!/storm-c|storm-d|storm-e/.test(failed), 'and only that one', failed.slice(0, 400))
  await t.shot(page, 'verification-filter')

  t.check(await t.open(page, '#/node/storm-b', /Last verification[\s\S]*Failed/i), "the failed node's page says so", (await t.text(page)).slice(0, 500))
  const b = await t.text(page)
  t.check(/stormlb:\/usr\/sbin\/stormlb/.test(b), 'names the failing asset')
  t.check(/expected e7db, found aa11/.test(b), 'what it expected and found')
  t.check(/initramfs/.test(b) && /missing/.test(b), 'and the missing one')
  t.check(!/registry-data/.test(b), 'not what is unverifiable by nature')
  t.check(/release 12\.61/.test(b), 'the release')
  t.check(!/golden/i.test(b), 'in the console\'s words', (b.match(/.{0,30}golden.{0,30}/i) || [''])[0])
  await t.shot(page, 'verification-failed')

  t.check(await t.open(page, '#/node/storm-d', /first pass is running/), 'before the first pass: says so')
  t.check(await t.open(page, '#/node/storm-e', /did not answer on :9103/), 'no sectionsystems: says so')
  t.check(!/Last verification[\s\S]*Failed/.test(await t.text(page)), 'and not as a failure')
  await t.shot(page, 'verification-silent')
  t.check(await t.open(page, `#/node/${HOST}`, /Last verification/), 'this node, from the real one')
  await t.shot(page, 'verification-real')
})
