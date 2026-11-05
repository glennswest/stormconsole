// What a container or VM disk runs on, in a browser (#75), over deploy/verify-runs-on.sh's
// real engine and apiserver.
const { walk } = require('./lib.cjs')
const { IMG, OLD, NEW, ROOT } = process.env

walk('runs-on', async (t) => {
  const page = await t.page('root')
  t.check(await t.open(page, '#/pod/shop/web', /Runs on/), 'the pod page says what it runs on', (await t.text(page)).slice(0, 400))
  await page.waitForTimeout(1500)
  let text = await t.text(page)
  t.check(text.includes('ctr-web-app') && text.includes(ROOT), 'the volume, name and id')
  t.check(/written of/.test(text), 'what it wrote of its size', (text.match(/.{0,60}written of.{0,60}/) || [''])[0])
  t.check(/\d+ slabs?/.test(text), 'the slabs')
  t.check(text.includes(IMG), 'cloned from the sealed image')
  t.check(/sha256:[0-9a-f]{64}/.test(text), 'its digest')
  t.check(/intact[\s\S]{0,12}\d+[smhd] ago, this node's copy/i.test(text), 'last verified, intact, this copy', (text.match(/.{0,30}intact.{0,60}/i) || [''])[0])
  t.check(text.includes(OLD), 'the image the node names')
  t.check(!text.includes(NEW), 'not the newest built')
  t.check(!/not confirmed running/.test(await page.content()), 'and not hedged')
  await t.shot(page, 'runs-on-pod')

  t.check(await t.open(page, '#/pod/shop/quiet', /Images/), 'a container the node said nothing of')
  await page.waitForTimeout(1000)
  text = await t.text(page)
  t.check(/has not said which volume/.test(text), 'says the node has not said')
  t.check(text.includes(NEW) && /not confirmed running/.test(await page.content()), 'the newest, hedged')
  await t.shot(page, 'runs-on-quiet')

  t.check(await t.open(page, '#/pod/shop/elsewhere', /holds no volume/), 'a volume on another engine: said')
  text = await t.text(page)
  t.check(/that node's console can say/.test(text), 'and whose it is to say', text.slice(0, 300))
  await t.shot(page, 'runs-on-elsewhere')

  t.check(await t.open(page, '#/vm/shop/vm1', /shop\.vm1-root/), "the VM page's disks")
  text = await t.text(page)
  t.check(/Each disk's instance/i.test(text), 'a disk is an instance there')
  t.check(text.includes('shop.vm1-data') && !text.includes('lab.vm1-root'), "this machine's, not another project's")
  t.check(/nothing sealed here/.test(text), 'an empty disk says so')
  t.check(text.includes(IMG), 'the image of the other')
  await t.shot(page, 'runs-on-vm')
})
