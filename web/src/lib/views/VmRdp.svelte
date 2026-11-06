<script>
  // RDP to a machine (#54): IronRDP's web component, at the versions stormrdp pins, loaded only when this is used.
  // The console trades the viewer's bearer for a login and relays the socket, so the password is only ever here.
  import { onDestroy } from 'svelte'
  import { get, postJson } from '../api.js'

  let { ns, name } = $props()

  const base = $derived(`/api/plugins/vm/vms/${encodeURIComponent(ns)}/${encodeURIComponent(name)}/rdp`)
  let status = $state(null)
  let phase = $state('idle')
  let said = $state('')
  let host = $state(null)
  let ui = null
  let leaving = false

  $effect(() => {
    get(base).then((s) => (status = s)).catch((e) => (status = { available: false, reason: e.message }))
  })

  async function client() {
    if (ui) return ui
    await import('@devolutions/iron-remote-desktop')
    const { Backend, init } = await import('@devolutions/iron-remote-desktop-rdp')
    await init('WARN')
    const element = document.createElement('iron-remote-desktop')
    element.setAttribute('scale', 'fit')
    element.setAttribute('flexcenter', 'true')
    element.module = Backend
    const ready = new Promise((resolve) => element.addEventListener('ready', (e) => resolve(e.detail.irgUserInteraction), { once: true }))
    host.append(element)
    ui = await ready
    return ui
  }

  async function connect() {
    phase = 'connecting'
    said = ''
    leaving = false
    try {
      const login = await postJson(`${base}/login`, {})
      const rdp = await client()
      const { displayControl } = await import('@devolutions/iron-remote-desktop-rdp')
      const size = host.getBoundingClientRect()
      const scheme = location.protocol === 'https:' ? 'wss' : 'ws'
      const config = rdp
        .configBuilder()
        .withUsername(login.login)
        .withPassword(login.password)
        .withDestination(login.target)
        .withProxyAddress(`${scheme}://${location.host}${base}/session`)
        .withAuthToken('stormrdp')
        .withServerDomain('')
        .withDesktopSize({ width: Math.floor(size.width), height: Math.floor(size.height) })
        .withExtension(displayControl(true))
        .build()
      const session = await rdp.connect(config)
      rdp.setVisibility(true)
      phase = 'connected'
      const end = await session.run()
      said = leaving ? 'Disconnected.' : `The session ended: ${end.reason()}`
    } catch (e) {
      said = leaving ? 'Disconnected.' : failure(e)
    }
    ui?.setVisibility(false)
    phase = 'idle'
  }

  // IronRDP's errors are its own: a refused login at NLA is "[CredSSP @ …] access denied", and the page says what
  // that means here, with its words after.
  function failure(e) {
    const text = e?.backtrace?.() ?? e?.message ?? String(e)
    if (phase === 'connected') return `The session ended: ${text}`
    if (/access denied/i.test(text)) {
      return `The gateway refused you (${text}). It asks the apiserver at each connection whether you may get this machine's instance in ${ns}, and that login may also have been replaced by a newer one.`
    }
    return `Could not connect: ${text}`
  }

  function disconnect() {
    leaving = true
    ui.shutdown()
  }

  onDestroy(() => {
    if (phase !== 'idle') ui?.shutdown()
  })
</script>

<div class="rdp" data-testid="rdp">
  <div class="bar">
    <span class="state {phase}">{phase}</span>
    {#if phase === 'connected'}
      <button type="button" onclick={() => ui.ctrlAltDel()}>Ctrl+Alt+Del</button>
      <button type="button" onclick={disconnect}>Disconnect</button>
    {:else}
      <button type="button" class="sc-primary" disabled={!status?.available || phase === 'connecting'} onclick={connect}>Connect</button>
    {/if}
    {#if status?.target && phase === 'idle'}<span class="dim mono">{status.target}</span>{/if}
  </div>
  {#if status && !status.available}<p class="dim" data-testid="rdp-why">{status.reason}</p>{/if}
  {#if said}<p class="said" data-testid="rdp-said">{said}</p>{/if}
  {#if status?.available}<p class="dim note">A login here replaces your earlier remote desktop login, so another client using it has to log in again.</p>{/if}
  <div class="screen" class:shown={phase !== 'idle'} bind:this={host}></div>
</div>

<style>
  .rdp { display: grid; gap: 10px; }
  .bar { display: flex; flex-wrap: wrap; gap: 10px; align-items: center; }
  .state { font-size: var(--sc-t-meta); color: var(--text-dim); text-transform: capitalize; }
  .state.connected { color: var(--ok-strong, var(--text)); }
  .dim { color: var(--text-dim); font-size: var(--sc-t-meta); margin: 0; }
  .mono { font-family: var(--mono); }
  .said { margin: 0; font-size: var(--sc-t-body); }
  .note { font-size: var(--sc-t-eyebrow); }
  .screen { height: 0; overflow: hidden; background: #000; border-radius: var(--radius); }
  .screen.shown { height: min(72vh, 900px); }
</style>
