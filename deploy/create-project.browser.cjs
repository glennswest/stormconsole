// The browser half of deploy/verify-create-project.sh (#56): a person
// opening Create → Virtual machine, choosing "+ New project…", naming it,
// and creating — twice: once with no projects yet (the dialog starts on New
// project), once with one (New project chosen from the list). Any page
// error fails the run, because an uncaught one is what stops the whole
// console from answering.
const { chromium } = require('playwright')

const BASE = process.env.CONSOLE
let failed = 0
const errors = []
function check(ok, what, extra = '') {
  console.log(`  ${ok ? 'ok  ' : 'FAIL'} ${what}${extra ? ` — ${extra}` : ''}`)
  if (!ok) failed++
}
const pause = (ms) => new Promise((r) => setTimeout(r, ms))

;(async () => {
  const browser = await chromium.launch()
  const ctx = await browser.newContext()
  const page = await ctx.newPage()
  page.on('pageerror', (e) => errors.push(`pageerror: ${e.message.split('\n')[0]}`))
  page.on('console', (m) => {
    if (m.type() === 'error') errors.push(`console: ${m.text().split('\n')[0]}`)
  })

  const login = await ctx.request.post(`${BASE}/api/v1/auth/login`, {
    data: { username: 'alice', password: 'pw' },
  })
  check(login.ok(), 'signed in as alice', `${login.status()}`)

  const dialog = page.locator('[role=dialog]')
  const projectSel = dialog.locator('select[aria-label=Project]')
  const newName = dialog.locator('input[placeholder^=lowercase]')
  const field = (label) =>
    dialog.locator('label', { has: page.locator('.lbl', { hasText: new RegExp(`^${label}\\*?$`) }) }).locator('input')

  async function openCreateVm() {
    await page.goto(`${BASE}/#/vms`)
    await page.waitForLoadState('networkidle')
    // The page's own Create menu: two creators here, so a menu.
    await page.locator('main button.create, .create').last().click()
    await page.getByRole('menuitem').filter({ hasText: /^Virtual machine\s*vm$/ }).click()
    await dialog.waitFor({ timeout: 5000 })
  }

  async function nameAndCreate(project, vm) {
    check(await newName.isVisible(), 'the new-project name box is shown')
    await newName.fill(project)
    await pause(4000) // long enough for a reload, a poll or a loop to undo it
    check((await projectSel.inputValue()) === '__new', 'the picker still says New project', await projectSel.inputValue())
    check(await newName.isVisible(), 'the name box is still there')
    check((await newName.inputValue().catch(() => '')) === project, `it still holds "${project}"`, await newName.inputValue().catch((e) => e.message))
    await field('Name').fill(vm)
    await field('Root disk').fill('rocky')
    await dialog.getByRole('button', { name: /^Create Virtual machine$/ }).click()
    const msg = await dialog.locator('.msg').first().textContent({ timeout: 15000 }).catch(() => '(no answer)')
    check(/created/i.test(msg) && !(await dialog.locator('.msg.err').count()), `created ${vm} in ${project}`, msg.trim())
    await dialog.waitFor({ state: 'detached', timeout: 5000 }).catch(() => {})
    check(!(await dialog.count()), 'the dialog closed')
  }

  try {
    console.log('\n--- 1. no projects yet: the dialog starts on New project')
    await openCreateVm()
    await pause(2000)
    check((await projectSel.inputValue()) === '__new', 'the picker starts on New project', await projectSel.inputValue())
    check((await newName.inputValue().catch(() => '')) === 'alice-vms', 'a name is suggested', await newName.inputValue().catch(() => ''))
    await nameAndCreate('alice-vms', 'vm1')

    console.log('\n--- 2. one project: choose New project from the list')
    await openCreateVm()
    await pause(2000)
    check((await projectSel.inputValue()) === 'alice-vms', 'the picker starts on her project', await projectSel.inputValue())
    await projectSel.selectOption('__new')
    await pause(4000)
    check((await projectSel.inputValue()) === '__new', 'New project stays chosen', await projectSel.inputValue())
    await nameAndCreate('alice-lab', 'vm2')

    console.log('\n--- 3. the rest of the console still answers')
    await page.goto(`${BASE}/#/projects`)
    await page.waitForLoadState('networkidle')
    await pause(1500)
    const body = await page.locator('body').innerText()
    check(body.includes('alice-vms') && body.includes('alice-lab'), 'Projects lists both')
    await openCreateVm()
    await dialog.getByRole('button', { name: 'Cancel' }).click()
    await pause(500)
    check(!(await dialog.count()), 'the dialog opens and cancels again')
  } catch (e) {
    check(false, 'the walk finished', e.message.split('\n')[0])
  }

  check(errors.length === 0, 'no page errors', errors.slice(0, 5).join(' | '))
  await browser.close()
  console.log(failed ? `\n${failed} failed` : '\nall passed')
  process.exit(failed ? 1 : 0)
})()
