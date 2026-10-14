<script>
  import LoginPanel from 'stormview/components/LoginPanel.svelte'
  import { nav, auth, login } from '../stores.svelte.js'

  // A token sign-in (#66) is the install-time API token — storminstall's
  // `apiToken`, given at the end of `storminstall wait` — or the console's
  // own bearer. It is the first screen when there are no named users,
  // which is a freshly installed node.
  const offers = $derived(auth.signIn)
  let mode = $state(auth.signIn.includes('password') ? 'password' : 'token')
</script>

{#key mode}
  <LoginPanel
    title={nav.name}
    subtitle={mode === 'token' ? 'Sign in with the API token' : 'Sign in to the StormCOS console'}
    askUsername={mode === 'password'}
    onsubmit={(username, password) => login(mode === 'token' ? '' : username, password)}
  />
{/key}

{#if offers.includes('password') && offers.includes('token')}
  <div class="switch">
    {#if mode === 'password'}
      <button type="button" onclick={() => (mode = 'token')}>Sign in with the API token instead</button>
    {:else}
      <button type="button" onclick={() => (mode = 'password')}>Sign in with a user name instead</button>
    {/if}
  </div>
{/if}
{#if mode === 'token'}
  <p class="hint">
    The <code>apiToken</code> from install-config.yaml, printed at the end of <code>storminstall wait</code>.
  </p>
{/if}

<style>
  .switch,
  .hint {
    position: fixed;
    left: 50%;
    transform: translateX(-50%);
    width: min(420px, 92vw);
    text-align: center;
    margin: 0;
  }
  .switch { bottom: 56px; }
  .hint {
    bottom: 24px;
    font-size: 12px;
    color: var(--text-faint);
  }
  .switch button {
    background: none;
    border: none;
    color: var(--text-dim);
    font-size: 12.5px;
    cursor: pointer;
    font-family: var(--font);
  }
  .switch button:hover { color: var(--accent); }
  code { font-family: var(--mono); }
</style>
