<script>
  // One virtual machine, and its two doors (#2).
  //
  // The doors are websockets on the console's own origin, addressed by VM
  // rather than node — so the browser never learns a node address, and the
  // URL survives a live migration (the client sees one reconnect, not a
  // new address). Neither upstream serves them yet, and this page says
  // which one is missing rather than showing a terminal that will never
  // print: an empty black rectangle is the worst possible answer to "is
  // my VM booting?".
  import { onDestroy } from 'svelte'
  import { route } from '../router.svelte.js'
  import { feed } from '../stores.svelte.js'
  import { get, call, wsUrl, ansiToHtml } from '../api.js'
  import PageHeader from '../components/PageHeader.svelte'
  import EmptyState from '../components/EmptyState.svelte'
  import StatusPill from '../components/StatusPill.svelte'
  import YamlPanel from '../components/YamlPanel.svelte'
  import Icon from '../components/Icon.svelte'

  const ns = $derived(route.current.params.ns)
  const name = $derived(route.current.params.name)

  const TABS = ['Overview', 'Serial console', 'Graphical console', 'YAML']
  let tab = $state('Overview')

  let vm = $state(null)
  let error = $state('')
  let loaded = $state(false)
  let acting = $state('')

  const component = $derived(
    feed.components.find((c) => c.id === `vm:instance:${ns}/${name}`) ||
      feed.components.find((c) => c.id === `vm:machine:${ns}/${name}`)
  )

  async function load() {
    loaded = false
    error = ''
    try {
      vm = await get(`/api/plugins/vm/vms/${encodeURIComponent(ns)}/${encodeURIComponent(name)}`)
    } catch (e) {
      error = e.message
      vm = null
    }
    loaded = true
  }

  $effect(() => {
    if (ns && name) load()
  })

  async function act(a) {
    if (a.danger && !confirm(`${a.label} ${name}?`)) return
    acting = a.id
    try {
      await call(a.method, a.path)
      await load()
    } catch (e) {
      error = e.message
    }
    acting = ''
  }

  // --- serial ---------------------------------------------------------
  // A serial console is bytes both ways. The output is rendered through
  // the same ANSI conversion the log tail uses, and keystrokes go back as
  // they are typed — nothing here interprets the stream.

  let serialSocket = null
  // What has arrived since the last frame, and the frame that will apply it.
  let pending = []
  let flush = null
  let serialText = $state('')
  let serialState = $state('idle')
  let serialBox = $state(null)
  let serialFocused = $state(false)

  // One frame's worth of output, applied together.
  //
  // The tail is kept rather than the whole stream: a console is for watching
  // what is happening, and an archive of the boot is the log the hypervisor
  // is already writing. Trimming on a character count would cut mid-escape
  // and leave the terminal in a colour; trimming to a line boundary does not.
  const SERIAL_MAX = 200000
  function applyPending() {
    flush = null
    if (!pending.length) return
    let next = serialText + pending.join('')
    pending = []
    if (next.length > SERIAL_MAX) {
      const cut = next.length - SERIAL_MAX
      const nl = next.indexOf('\n', cut)
      next = next.slice(nl === -1 ? cut : nl + 1)
    }
    serialText = next
    // After the render, not before it: scrolling to a height the browser has
    // not laid out yet leaves the view one frame short of the bottom, which
    // reads as a console that stops just before the line you want.
    // scrollTop alone: `scrollTo(0, h)` also sets scrollLeft to 0, which
    // would drag the view back to the left edge every frame now that there
    // is somewhere to scroll sideways to.
    queueMicrotask(() => {
      if (serialBox) serialBox.scrollTop = serialBox.scrollHeight
    })
  }

  function openSerial() {
    if (serialSocket) return
    serialState = 'connecting'
    const s = new WebSocket(
      wsUrl(`/api/plugins/vm/console/${encodeURIComponent(ns)}/${encodeURIComponent(name)}/serial`)
    )
    s.binaryType = 'arraybuffer'
    s.onopen = () => {
      serialState = 'open'
      // Focus on connect.
      //
      // Keystrokes only reach the socket when this div has focus, and
      // nothing took it or said so — so connecting, typing, and seeing
      // nothing happen was the expected experience. It reads as a console
      // with no echo, which is a very different bug from the one it is.
      queueMicrotask(() => serialBox?.focus())
    }
    s.onmessage = (e) => {
      const chunk =
        typeof e.data === 'string' ? e.data : new TextDecoder().decode(new Uint8Array(e.data))
      // Batched to a frame, not applied per message.
      //
      // This concatenated a 200 KB string, re-sliced it and re-rendered the
      // whole buffer on *every* chunk. A kernel boot emits hundreds of small
      // writes a second, so the work per second grew with the length of the
      // log — the console fell further behind the longer it watched, which
      // is exactly when you are watching it.
      //
      // Now the chunks pile up in an array and one frame's worth is applied
      // at a time: at most ~60 renders a second whatever the guest does, and
      // the concatenation happens once per frame over what arrived in it
      // rather than once per message over everything so far.
      pending.push(chunk)
      if (flush === null) flush = requestAnimationFrame(applyPending)
    }
    s.onclose = () => {
      serialState = 'closed'
      serialSocket = null
    }
    s.onerror = () => (serialState = 'closed')
    serialSocket = s
  }

  function closeSerial() {
    serialSocket?.close()
    serialSocket = null
    serialState = 'idle'
    // A frame still queued against a closed socket would apply one more
    // batch and then hold a reference to it.
    if (flush !== null) cancelAnimationFrame(flush)
    flush = null
    pending = []
  }

  function typeInto(e) {
    if (!serialSocket || serialSocket.readyState !== 1) return
    // A read-only viewer is stopped here as well as at the relay. The
    // relay is the boundary that matters — this one only exists so a
    // keystroke does not look like it went somewhere and was ignored.
    if (!doors.write) {
      e.preventDefault()
      return
    }
    let out = e.key
    if (e.key === 'Enter') out = '\r'
    else if (e.key === 'Backspace') out = '\x7f'
    else if (e.key === 'Tab') out = '\t'
    else if (e.key === 'Escape') out = '\x1b'
    else if (e.key.length !== 1) return
    else if (e.ctrlKey) {
      const c = e.key.toLowerCase().charCodeAt(0) - 96
      if (c < 1 || c > 26) return
      out = String.fromCharCode(c)
    }
    e.preventDefault()
    serialSocket.send(out)
  }

  // --- framebuffer ----------------------------------------------------
  // noVNC talks RFB over the same kind of socket; it is loaded only when
  // the tab is opened, so the 350 KB is not in the console's first paint.

  let vncBox = $state(null)
  let vncState = $state('idle')
  let rfb = null

  async function openVnc() {
    if (rfb || !vncBox) return
    vncState = 'connecting'
    try {
      const { default: RFB } = await import('@novnc/novnc')
      rfb = new RFB(
        vncBox,
        wsUrl(`/api/plugins/vm/console/${encodeURIComponent(ns)}/${encodeURIComponent(name)}/vnc`)
      )
      rfb.scaleViewport = true
      rfb.addEventListener('connect', () => (vncState = 'open'))
      rfb.addEventListener('disconnect', () => {
        vncState = 'closed'
        rfb = null
      })
    } catch (e) {
      vncState = 'closed'
      error = `framebuffer: ${e.message}`
    }
  }

  function closeVnc() {
    try {
      rfb?.disconnect()
    } catch {}
    rfb = null
    vncState = 'idle'
  }

  onDestroy(() => {
    closeSerial()
    closeVnc()
  })

  const doors = $derived(
    vm?.console || { serial: false, vnc: false, replay: false, write: false, reason: '' }
  )
</script>

<div class="sc-page">
  <PageHeader
    crumbs={[
      { label: 'Virtualization' },
      { label: 'Virtual machines', href: '#/vms' },
      { label: name },
    ]}
    title={name}
    scope={`in ${ns}`}
  >
    {#snippet status()}
      {#if component}<StatusPill health={component.health} />{/if}
    {/snippet}
    {#snippet actions()}
      {#each component?.actions || [] as a}
        <button class:danger={a.danger} disabled={!a.enabled || acting === a.id} onclick={() => act(a)}>
          {a.label}
        </button>
      {/each}
    {/snippet}
  </PageHeader>

  {#if !loaded}
    <div class="sc-empty"><p>Loading {name}…</p></div>
  {:else if error && !vm}
    <EmptyState icon="pod" title="This VM cannot be read" hint={error}>
      {#snippet action()}
        <a class="sc-back" href="#/vms">Back to virtual machines</a>
      {/snippet}
    </EmptyState>
  {:else}
    <nav class="tabs" aria-label="Virtual machine sections">
      {#each TABS as t}
        <button
          class:active={tab === t}
          aria-current={tab === t ? 'page' : undefined}
          onclick={() => {
            tab = t
            if (t === 'Serial console' && doors.serial) openSerial()
            if (t === 'Graphical console' && doors.vnc) queueMicrotask(openVnc)
          }}
        >{t}</button>
      {/each}
    </nav>

    {#if error}<p class="error">{error}</p>{/if}

    {#if tab === 'Overview'}
      <section class="cards">
        <div class="card">
          <h2>Machine</h2>
          <dl>
            <dt>State</dt><dd>{vm.phase || (vm.hasDefinition ? 'stopped' : 'unknown')}</dd>
            <dt>Node</dt><dd class="mono">{vm.node || '—'}</dd>
            <dt>vCPU</dt><dd class="mono">{vm.vcpu ?? '—'}</dd>
            <dt>Memory</dt><dd class="mono">{vm.memory || '—'}</dd>
            <dt>Definition</dt>
            <dd>{vm.hasDefinition ? 'a VirtualMachine defines it' : 'an instance applied on its own'}</dd>
          </dl>
          {#if vm.reason}
            <p class="reason">{vm.reason}</p>
          {/if}
        </div>

        <div class="card">
          <h2>Disks</h2>
          {#if !vm.disks?.length}
            <p class="none">No disks. A VM with no root disk has nothing to boot.</p>
          {:else}
            <ul class="rows">
              {#each vm.disks as d (d.name)}
                <li><span class="mono">{d.name}</span><span class="dim">{d.backing}</span></li>
              {/each}
            </ul>
          {/if}
        </div>

        <div class="card">
          <h2>Network</h2>
          {#if !vm.networks?.length}
            <p class="none">No networks.</p>
          {:else}
            <ul class="rows">
              {#each vm.networks as n, i (n.name || i)}
                <li>
                  <span class="mono">{n.name}</span>
                  <span class="dim">{Object.keys(n).filter((k) => k !== 'name').join(', ') || '—'}</span>
                </li>
              {/each}
            </ul>
          {/if}
        </div>
      </section>
    {:else if tab === 'Serial console'}
      {#if !doors.serial}
        <EmptyState icon="logs" title="No serial console yet" hint={doors.reason} />
      {:else}
        <div class="console">
          <div class="bar">
            <span class="state {serialState}">{serialState}</span>
            <span class="hint">
              {#if !doors.write}
                Read-only — you can watch this console, not type into it.
              {:else}
                Click the terminal and type — keys go straight to the guest.
              {/if}
            </span>
            <span class="right">
              {#if serialState === 'open'}
                <button onclick={closeSerial}>Disconnect</button>
              {:else}
                <button class="sc-primary" onclick={openSerial}>Connect</button>
              {/if}
            </span>
          </div>
          <div
            class="term"
            class:focused={serialFocused}
            bind:this={serialBox}
            tabindex="0"
            role="textbox"
            aria-label="Serial console for {name}"
            onkeydown={typeInto}
            onfocus={() => (serialFocused = true)}
            onblur={() => (serialFocused = false)}
          >{@html ansiToHtml(serialText)}</div>
          <!-- Whether typing goes anywhere, said out loud. -->
          <!-- Where the history ends and the live stream begins.
               stormvm sends the tail of the guest's own console log on
               attach (64 KiB, cut at a line boundary), so a console opened
               ten minutes into a boot prints the boot. A reader who does
               not know that is reading history as though it were now. -->
          <p class="termhint">
            {#if serialState !== 'open'}
              Not connected.
            {:else if !doors.write}
              Read-only. This console shows what the guest is printing; typing
              into it needs the <code>operator</code> role, because a serial
              line is a root shell on most guests.
            {:else if serialFocused}
              Typing goes to the guest. Echo comes back from it — a guest with
              no getty on its serial line will show nothing.{#if doors.replay}
                The first screenful is replayed from the guest's console log,
                not live.{/if}
            {:else}
              Click the console to type into it.
            {/if}
          </p>
        </div>
      {/if}
    {:else if tab === 'Graphical console'}
      {#if !doors.vnc}
        <EmptyState
          icon="overview"
          title="No framebuffer yet"
          hint="{doors.reason} A graphical console is what a Windows installer and anything else with a screen needs, so it lands with stormvm's VNC door."
        />
      {:else}
        <div class="console">
          <div class="bar">
            <span class="state {vncState}">{vncState}</span>
            <span class="right">
              {#if vncState === 'open'}
                <button onclick={closeVnc}>Disconnect</button>
              {:else}
                <button class="sc-primary" onclick={openVnc}>Connect</button>
              {/if}
            </span>
          </div>
          <div class="fb" bind:this={vncBox}></div>
        </div>
      {/if}
    {:else}
      <YamlPanel yaml={vm.yaml} label="{name} in {ns}" />
    {/if}
  {/if}
</div>

<style>
  .tabs {
    display: flex;
    gap: 2px;
    border-bottom: 1px solid var(--border);
    margin-bottom: 14px;
  }
  .tabs button {
    background: none;
    border: none;
    border-bottom: 2px solid transparent;
    border-radius: 0;
    padding: 7px 14px;
    font-size: var(--sc-t-body);
    color: var(--text-dim);
  }
  .tabs button:hover { color: var(--text); background: var(--nav-hover); }
  .tabs button.active { color: var(--text); border-bottom-color: var(--accent); font-weight: 600; }

  .error {
    margin: 0 0 12px;
    font-size: var(--sc-t-body);
    color: var(--error);
  }

  .cards {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(260px, 1fr));
    gap: 12px;
    align-items: start;
  }
  .card {
    background: var(--panel);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    padding: 14px var(--sc-row-px);
  }
  h2 {
    font-size: var(--sc-t-eyebrow);
    text-transform: uppercase;
    letter-spacing: 0.06em;
    color: var(--text-faint);
    margin: 0 0 10px;
  }
  dl { display: grid; grid-template-columns: auto 1fr; gap: 4px 14px; margin: 0; }
  dt { font-size: var(--sc-t-meta); color: var(--text-faint); }
  dd { margin: 0; font-size: var(--sc-t-body); }
  .mono { font-family: var(--mono); font-size: var(--sc-t-meta); }
  .none { margin: 0; font-size: var(--sc-t-body); color: var(--text-dim); }
  .reason {
    margin: 10px 0 0;
    font-size: var(--sc-t-meta);
    color: var(--warn-strong);
  }
  .rows { list-style: none; display: grid; gap: 4px; }
  .rows li { display: flex; gap: 10px; align-items: baseline; }
  .dim { color: var(--text-dim); font-size: var(--sc-t-meta); }

  .console {
    background: var(--panel);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    overflow: hidden;
  }
  .bar {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 7px var(--sc-row-px);
    border-bottom: 1px solid var(--border);
    background: color-mix(in srgb, var(--panel-raised) 55%, var(--panel));
  }
  .state {
    font-size: var(--sc-t-eyebrow);
    text-transform: uppercase;
    letter-spacing: 0.06em;
    color: var(--text-faint);
  }
  .state.open { color: var(--ok); }
  .state.closed { color: var(--error); }
  .hint { font-size: var(--sc-t-meta); color: var(--text-faint); }
  .right { margin-left: auto; }
  .right button { font-size: var(--sc-t-meta); padding: 3px 10px; }

  .term {
    /* Taller, because a console is the view you sit and watch. */
    height: 78vh;
    overflow: auto;
    padding: 10px var(--sc-row-px);
    font-family: var(--mono);
    font-size: var(--sc-t-meta);
    line-height: 1.4;
    /* `pre`, not `pre-wrap`: wrapping breaks alignment, and alignment is
       most of what serial output is for. `dmesg`, a partition table, a
       systemd boot -- all of them are columns, and a wrapped line silently
       becomes two and stops lining up with the ones around it. Long lines
       scroll sideways instead. */
    white-space: pre;
    background: #000;
    color: #d0d0d0;
    outline: none;
  }
  .term:focus { box-shadow: inset 0 0 0 2px var(--accent); }
  .termhint {
    margin: 6px 0 0;
    font-size: var(--sc-t-meta);
    color: var(--muted);
  }
  .fb { height: 62vh; background: #000; }
  .sc-back { font-size: var(--sc-t-body); }
</style>
