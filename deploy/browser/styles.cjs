// The masthead's style selector (#151): ContainerFocus, no trademark names,
// and a choice stored before the rename still opens in its style.
const { walk } = require('./lib.cjs')

const KEY = 'stormconsole-style'
const style = (page) => page.evaluate(() => document.documentElement.getAttribute('data-style'))
const picker = (page) => page.locator('select[aria-label="Console style"]')

walk('styles', async (t) => {
  const page = await t.page()
  await page.goto(`${t.BASE}/#/`)
  await t.until(async () => (await picker(page).count()) > 0)

  // A first visit: the default.
  const labels = await picker(page).locator('option').allInnerTexts()
  t.check(labels.includes('ContainerFocus'), 'the selector offers ContainerFocus', labels.join(', '))
  t.check(!labels.some((l) => /openshift/i.test(l)), 'and not OpenShift', labels.join(', '))
  t.check((await style(page)) === 'containerfocus', 'a first visit opens in containerfocus', await style(page))
  t.check((await picker(page).inputValue()) === 'containerfocus', 'and the selector says so')
  await t.shot(page, 'style-default')

  // A choice stored under the old id, as an older console saved it.
  await page.evaluate((k) => localStorage.setItem(k, JSON.stringify('openshift')), KEY)
  await page.reload()
  await t.until(async () => (await picker(page).count()) > 0)
  t.check((await style(page)) === 'containerfocus', 'a stored "openshift" opens in containerfocus', await style(page))
  t.check((await picker(page).inputValue()) === 'containerfocus', 'the selector reads ContainerFocus')
  const saved = await page.evaluate((k) => localStorage.getItem(k), KEY)
  t.check(saved === '"containerfocus"', 'and the stored choice is rewritten', saved)
  const masthead = await page.evaluate(() => getComputedStyle(document.documentElement).getPropertyValue('--nav-h').trim())
  t.check(masthead === '48px', "with containerfocus's proportions", masthead)

  // The other style still switches and comes back.
  const other = (await picker(page).locator('option').evaluateAll((os) => os.map((o) => o.value))).find((v) => v !== 'containerfocus')
  await picker(page).selectOption(other)
  t.check((await style(page)) === other, `switching to ${other} works`, await style(page))
  await page.reload()
  await t.until(async () => (await picker(page).count()) > 0)
  t.check((await style(page)) === other, `${other} is kept across a reload`)
  await picker(page).selectOption('containerfocus')
  t.check((await style(page)) === 'containerfocus', 'and back to containerfocus')

  // Nothing a person can read on the page names OpenShift: text, titles,
  // labels, placeholders, options.
  const words = await page.evaluate(() => {
    const out = [document.body.innerText, document.title]
    for (const e of document.querySelectorAll('*')) {
      for (const a of ['title', 'aria-label', 'placeholder', 'alt']) if (e.getAttribute(a)) out.push(e.getAttribute(a))
      if (e.tagName === 'OPTION') out.push(e.textContent)
    }
    return out.join('\n')
  })
  const hit = words.split('\n').filter((l) => /openshift/i.test(l))
  t.check(hit.length === 0, 'no visible "OpenShift" on the overview', hit.join(' | '))
})
