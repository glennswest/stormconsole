<script>
  // Machines (#31): bare metal by service tag, from stormipmi's Machines API.
  //
  // One row per machine: what its BMC is, whether it is on (what the BMC
  // last said, not what was asked), the release it boots from the forge,
  // whether it is a test machine, and its serial console. Then the default
  // image a new tag boots, and the hosts the forge has seen that nothing
  // manages yet, to adopt.
  //
  // Every change is an administrator's, and the plugin enforces it; the page
  // only decides whether to offer the buttons. Repointing a release takes
  // effect at the machine's next boot — nothing is power-cycled — and
  // stormipmi answers only once it has read the change back.
  import { onDestroy } from 'svelte'
  import { get, call, postJson, wsUrl, ansiToHtml } from '../api.js'
  import { noteActivity } from '../stores.svelte.js'
  import PageHeader from '../components/PageHeader.svelte'
  import EmptyState from '../components/EmptyState.svelte'
  import CopyButton from '../components/CopyButton.svelte'

  const API = '/api/plugins/ipmi/proxy/api/v1'

  let fleet = $state(null)
  let releases = $state([])
  let me = $state({ admin: false, why: '' })
  let error = $state('')
  let saved = $state('')
  let busy = $state('')
  let testOnly = $state(false)
  let search = $state('')
  let timer = null

  async function load() {
    try {
      fleet = await get(`${API}/machines${testOnly ? '?test=true' : ''}`)
      error = ''
    } catch (e) {
      error = `stormipmi did not answer: ${e.message}. Is [stormipmi] url pointing at the node that runs it?`
    }
    clearTimeout(timer)
    timer = setTimeout(load, 5000)
  }
  $effect(() => {
    testOnly
    load()
  })
  $effect(() => {
    get(`${API}/releases`).then((d) => (releases = d.releases || [])).catch(() => {})
    get('/api/plugins/ipmi/me').then((d) => (me = d)).catch(() => {})
  })
  onDestroy(() => {
    clearTimeout(timer)
    closeConsole()
  })

  const machines = $derived(
    (fleet?.machines || []).filter(
      (m) => !search || `${m.tag} ${m.name} ${m.node || ''} ${m.bmc?.address || ''}`.toLowerCase().includes(search.toLowerCase())
    )
  )
  const managed = $derived(machines.filter((m) => !m.adopt))
  const toAdopt = $derived(machines.filter((m) => m.adopt))

  async function act(label, fn, question) {
    if (question && !confirm(question)) return
    busy = label
    error = ''
    saved = ''
    try {
      const r = await fn()
      saved = r?.message || `${label}: done`
      noteActivity({ reason: label, message: saved, source: 'Machines' })
      await load()
    } catch (e) {
      error = e.message
      noteActivity({ reason: label, message: e.message, source: 'Machines', warning: true })
    }
    busy = ''
  }

  const POWER = [
    ['on', 'Power on', false],
    ['soft', 'Shut down (ACPI)', true],
    ['reboot', 'Reboot', true],
    ['off', 'Power off (hard)', true],
    ['cycle', 'Power cycle (hard)', true],
  ]
  function power(m, act_, label, danger) {
    act(label, () => call('POST', `${API}/machines/${encodeURIComponent(m.tag)}/power/${act_}`),
      danger ? `${label}: ${m.tag}${m.node ? ` (${m.node})` : ''}?` : null)
  }

  function setRelease(m, release) {
    if (!release || release === m.boot?.release) return
    act('Set release', () => postJson(`${API}/machines/${encodeURIComponent(m.tag)}/release`, { release }, 'PUT'),
      `Point ${m.tag} at release ${release}? It boots it at its next boot; nothing is power-cycled now.`)
  }
  function setDefault(release) {
    if (!release || release === fleet?.default?.release) return
    act('Default image', () => postJson(`${API}/machines/default`, { release }, 'PUT'),
      `Make ${release} what every newly seen machine boots?`)
  }
  function setTest(m, test) {
    act('Test mark', () => postJson(`${API}/machines/${encodeURIComponent(m.tag)}/test`, { test }, 'PUT'),
      test ? `Mark ${m.tag} as a test machine? stormcentral powers test machines off in the evening and installs on them.` : null)
  }

  // Adopt: a tag the forge has seen, given a BMC so stormipmi manages it.
  let adopting = $state('')
  let adopt = $state({ address: '', username: '', password: '', insecure: true, test: false })
  function startAdopt(m) {
    adopting = m.tag
    adopt = { address: '', username: '', password: '', insecure: true, test: false }
  }
  function doAdopt(m) {
    act('Adopt', async () => {
      const r = await postJson(`${API}/machines/${encodeURIComponent(m.tag)}/adopt`, {
        bmc: { address: adopt.address, username: adopt.username, password: adopt.password,
               disableCertificateVerification: adopt.insecure },
        test: adopt.test,
      })
      adopting = ''
      return { message: `${m.tag} adopted${r?.host ? ` as ${r.host.namespace}/${r.host.name}` : ''}` }
    })
  }

  // Boot intent: stormipmi answers 501 until the forge has intents
  // (stormblock#148), and the page says so rather than offering a no-op.
  let intents = $state({})
  async function readIntent(m) {
    try {
      const r = await fetch(`${API}/machines/${encodeURIComponent(m.tag)}/intent`)
      const d = await r.json().catch(() => ({}))
      intents[m.tag] = r.status === 501 ? { unsupported: true, why: d.error || 'the forge has no boot intents yet (stormblock#148)' } : d
    } catch (e) {
      intents[m.tag] = { unsupported: true, why: e.message }
    }
  }

  // --- the serial console ------------------------------------------------
  let consoleFor = $state(null)
  let consoleText = $state('')
  let consoleState = $state('idle')
  let line = $state('')
  let socket = null
  let screen = $state(null)
  const MAX = 200_000
  const decoder = new TextDecoder()

  function openConsole(m) {
    closeConsole()
    consoleFor = m
    consoleText = ''
    consoleState = 'connecting'
    socket = new WebSocket(wsUrl(`/api/plugins/ipmi/console/${encodeURIComponent(m.host.namespace)}/${encodeURIComponent(m.host.name)}`))
    socket.binaryType = 'arraybuffer'
    socket.onopen = () => (consoleState = 'open')
    socket.onmessage = (e) => {
      const t = typeof e.data === 'string' ? e.data : decoder.decode(new Uint8Array(e.data), { stream: true })
      consoleText = (consoleText + t).slice(-MAX)
      queueMicrotask(() => screen && (screen.scrollTop = screen.scrollHeight))
    }
    socket.onclose = () => (consoleState = 'closed')
    socket.onerror = () => (consoleState = 'closed')
  }
  function closeConsole() {
    try {
      socket?.close()
    } catch {}
    socket = null
  }
  function send() {
    if (socket?.readyState === 1) socket.send(new TextEncoder().encode(line + '\r'))
    line = ''
  }

  const when = (t) => (t ? new Date(typeof t === 'number' ? t * 1000 : t).toLocaleString() : '—')
  const gib = (mib) => (mib ? `${Math.round(mib / 1024)} GiB` : '')
</script>

<div class="sc-page">
  <PageHeader crumbs={[{ label: 'Hardware' }, { label: 'Machines' }]} title="Machines" count={fleet ? managed.length : null} />

  <div class="bar">
    <input bind:value={search} placeholder="Search by tag, name, node or BMC address" aria-label="Search" />
    <label class="check"><input type="checkbox" bind:checked={testOnly} /> test machines only</label>
    {#if !me.admin}<span class="dim">{me.why}</span>{/if}
  </div>

  {#if error}<p class="error">{error}</p>{/if}
  {#if saved}<p class="saved">{saved}</p>{/if}
  {#if fleet?.forge?.error}
    <p class="warn">The forge did not say what each machine boots: {fleet.forge.error}</p>
  {/if}

  {#if fleet && !managed.length && !toAdopt.length}
    <EmptyState icon="node" title="No machines" hint="stormipmi manages no BareMetalHost yet, and the forge has seen no new tags." />
  {/if}

  {#if managed.length}
    <table class="machines">
      <thead>
        <tr><th>Service tag</th><th>BMC</th><th>Power</th><th>Boots</th><th>Test</th><th></th></tr>
      </thead>
      <tbody>
        {#each managed as m (m.tag || m.name)}
          <tr class:err={!!m.error}>
            <td>
              <span class="mono strong">{m.tag || '(tag not read yet)'}</span>
              {#if m.tag}<CopyButton value={m.tag} label="Copy tag" />{/if}
              <div class="dim">{m.node || m.name}{m.host ? ` · ${m.host.namespace}/${m.host.name}` : ''}</div>
              <div class="dim">{[m.hardware?.vendor, m.hardware?.model].filter(Boolean).join(' ')}{m.hardware?.cpus ? ` · ${m.hardware.cpus} CPU` : ''}{m.hardware?.ramMebibytes ? ` · ${gib(m.hardware.ramMebibytes)}` : ''}</div>
              {#if m.error}<div class="bad">{m.error}</div>{/if}
            </td>
            <td>
              {#if m.bmc}
                <span class="mono">{m.bmc.address}</span>
                <div class="dim">{[m.bmc.vendor, m.bmc.model, m.bmc.firmware].filter(Boolean).join(' · ')}</div>
                <div class="dim">credentials: {m.bmc.credentialsName}</div>
              {:else}—{/if}
            </td>
            <td>
              <span class="power {m.power}">{m.power}</span>
              {#if m.online !== null && m.online !== undefined && (m.online ? 'on' : 'off') !== m.power && m.power !== 'unknown'}
                <div class="dim">asked: {m.online ? 'on' : 'off'}</div>
              {/if}
              <div class="dim">{m.state || ''}{m.lastSeen ? ` · seen ${when(m.lastSeen)}` : ''}</div>
            </td>
            <td>
              {#if m.boot}
                <span class="mono">{m.boot.release || '—'}</span>
                {#if m.boot.pinnedFromDefault}<span class="tagchip" title="set from the default image when this tag was first seen">from default</span>{/if}
                {#if m.boot.dangling}<div class="bad">points at a volume that is gone</div>{/if}
                <div class="dim">since {when(m.boot.since)}</div>
              {:else}<span class="dim">not on the forge</span>{/if}
              {#if me.admin && m.tag && releases.length}
                <select aria-label="Release for {m.tag}" disabled={!!busy}
                  onchange={(e) => { setRelease(m, e.target.value); e.target.value = '' }}>
                  <option value="">Set release…</option>
                  {#each releases as r (r.version)}<option value={r.version}>{r.version}{r.notes ? ` — ${r.notes}` : ''}</option>{/each}
                </select>
              {/if}
              {#if intents[m.tag]}
                <div class="dim">intent: {intents[m.tag].unsupported ? intents[m.tag].why : intents[m.tag].intent}</div>
              {:else if m.tag}
                <button class="link" onclick={() => readIntent(m)}>boot intent</button>
              {/if}
            </td>
            <td>
              {#if me.admin && m.host}
                <input type="checkbox" checked={m.test} disabled={!!busy} aria-label="Test machine" onchange={(e) => setTest(m, e.target.checked)} />
              {:else}{m.test ? 'yes' : ''}{/if}
            </td>
            <td class="acts">
              {#if m.host}<button onclick={() => openConsole(m)}>Console</button>{/if}
              {#if me.admin && m.host && m.tag}
                <select aria-label="Power {m.tag}" disabled={!!busy}
                  onchange={(e) => { const p = POWER.find((x) => x[0] === e.target.value); e.target.value = ''; if (p) power(m, ...p) }}>
                  <option value="">Power…</option>
                  {#each POWER as [a, label]}<option value={a}>{label}</option>{/each}
                </select>
              {/if}
            </td>
          </tr>
        {/each}
      </tbody>
    </table>
  {/if}

  {#if consoleFor}
    <section class="console">
      <div class="cbar">
        <span class="mono">{consoleFor.tag} · {consoleFor.host.namespace}/{consoleFor.host.name}</span>
        <span class="state {consoleState}">{consoleState}</span>
        <span class="dim">{me.admin ? 'you can type' : 'watching — typing is for administrators'}</span>
        <button onclick={() => { closeConsole(); consoleFor = null }}>Close</button>
      </div>
      <pre class="screen" bind:this={screen}>{@html ansiToHtml(consoleText)}</pre>
      {#if me.admin}
        <div class="cin">
          <input bind:value={line} placeholder="type a line, Enter sends it with a carriage return" aria-label="Console input"
            onkeydown={(e) => e.key === 'Enter' && send()} />
        </div>
      {/if}
    </section>
  {/if}

  {#if fleet}
    <section class="card">
      <h2>Default image</h2>
      <p>
        A tag seen for the first time boots
        <span class="mono strong">{fleet.default?.release || 'nothing — no default is set'}</span>.
      </p>
      {#if me.admin && releases.length}
        <select aria-label="Default release" disabled={!!busy} onchange={(e) => { setDefault(e.target.value); e.target.value = '' }}>
          <option value="">Change the default…</option>
          {#each releases as r (r.version)}<option value={r.version}>{r.version}</option>{/each}
        </select>
      {/if}
    </section>
  {/if}

  {#if toAdopt.length}
    <section class="card">
      <h2>New hosts to adopt</h2>
      <p class="dim">The forge has seen these tags boot; stormipmi does not manage them yet. Give each its BMC to manage its power and console.</p>
      <table class="machines">
        <tbody>
          {#each toAdopt as m (m.tag)}
            <tr>
              <td><span class="mono strong">{m.tag}</span></td>
              <td>{m.boot?.release ? `boots ${m.boot.release}` : ''}</td>
              <td class="acts">
                {#if me.admin}
                  {#if adopting === m.tag}
                    <div class="adopt">
                      <input bind:value={adopt.address} placeholder="BMC address, e.g. redfish://10.0.0.21" aria-label="BMC address" />
                      <input bind:value={adopt.username} placeholder="BMC user" aria-label="BMC user" />
                      <input bind:value={adopt.password} type="password" placeholder="BMC password" aria-label="BMC password" />
                      <label class="check"><input type="checkbox" bind:checked={adopt.insecure} /> self-signed BMC certificate</label>
                      <label class="check"><input type="checkbox" bind:checked={adopt.test} /> test machine</label>
                      <button class="sc-primary" disabled={!!busy || !adopt.address} onclick={() => doAdopt(m)}>Adopt</button>
                      <button onclick={() => (adopting = '')}>Cancel</button>
                    </div>
                  {:else}
                    <button onclick={() => startAdopt(m)}>Adopt…</button>
                  {/if}
                {/if}
              </td>
            </tr>
          {/each}
        </tbody>
      </table>
    </section>
  {/if}
</div>

<style>
  .bar { display: flex; gap: 12px; align-items: center; flex-wrap: wrap; margin-bottom: 12px; }
  .bar > input { width: 320px; }
  .check { display: inline-flex; gap: 6px; align-items: center; font-size: var(--sc-t-meta); }
  .check input { width: auto; }
  .error, .bad { color: var(--error); }
  .bad { font-size: var(--sc-t-meta); }
  .warn { color: var(--warn-strong); font-size: var(--sc-t-body); }
  .saved { color: var(--ok); }
  .dim { color: var(--text-dim); font-size: var(--sc-t-meta); }
  .mono { font-family: var(--mono); font-size: var(--sc-t-meta); }
  .strong { font-weight: 600; }
  table.machines { width: 100%; border-collapse: collapse; background: var(--panel); border: 1px solid var(--border); border-radius: var(--radius); margin-bottom: 12px; }
  table.machines th { text-align: left; font-size: var(--sc-t-eyebrow); text-transform: uppercase; letter-spacing: 0.05em; color: var(--text-faint); padding: 8px 12px; border-bottom: 1px solid var(--border); }
  table.machines td { padding: 8px 12px; font-size: var(--sc-t-body); vertical-align: top; }
  table.machines tr + tr > td { border-top: 1px solid var(--sc-hairline); }
  tr.err td:first-child { box-shadow: inset 3px 0 var(--error); }
  .power { font-family: var(--mono); font-size: var(--sc-t-meta); font-weight: 600; }
  .power.on { color: var(--ok); }
  .power.off { color: var(--text-dim); }
  .power.unknown { color: var(--warn-strong); }
  .tagchip { font-size: var(--sc-t-eyebrow); border: 1px solid var(--border); border-radius: var(--radius-sm); padding: 0 5px; margin-left: 6px; color: var(--text-dim); }
  .acts { white-space: nowrap; text-align: right; }
  .acts button, .acts select, td select { font-size: var(--sc-t-meta); margin: 2px 0 0 4px; }
  button.link { background: none; border: 0; padding: 0; color: var(--accent); font-size: var(--sc-t-meta); cursor: pointer; }
  .card { background: var(--panel); border: 1px solid var(--border); border-radius: var(--radius); padding: 14px var(--sc-row-px); margin-bottom: 12px; }
  h2 { font-size: var(--sc-t-eyebrow); text-transform: uppercase; letter-spacing: 0.06em; color: var(--text-faint); margin: 0 0 10px; }
  .adopt { display: flex; flex-wrap: wrap; gap: 6px; justify-content: flex-end; }
  .adopt input { width: 180px; }
  .console { border: 1px solid var(--border); border-radius: var(--radius); margin-bottom: 12px; background: #0b0d10; }
  .cbar { display: flex; gap: 12px; align-items: center; padding: 6px 10px; background: var(--panel); border-bottom: 1px solid var(--border); }
  .cbar button { margin-left: auto; font-size: var(--sc-t-meta); }
  .state { font-size: var(--sc-t-eyebrow); text-transform: uppercase; }
  .state.open { color: var(--ok); }
  .state.closed { color: var(--error); }
  .screen { margin: 0; padding: 8px 10px; height: 360px; overflow: auto; color: #d8dee9; font-family: var(--mono); font-size: 12px; white-space: pre-wrap; }
  .cin input { width: 100%; font-family: var(--mono); border: 0; border-top: 1px solid var(--border); border-radius: 0; }
</style>
