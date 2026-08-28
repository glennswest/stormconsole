<script>
  import { route } from './lib/router.svelte.js'
  import { auth, checkAuth, startFeed } from './lib/stores.svelte.js'
  import TopBar from './lib/components/TopBar.svelte'
  import Sidebar from './lib/components/Sidebar.svelte'
  import Overview from './lib/views/Overview.svelte'
  import GridView from './lib/views/GridView.svelte'
  import LogsView from './lib/views/LogsView.svelte'
  import Login from './lib/views/Login.svelte'

  checkAuth().then(() => {
    if (!auth.required || auth.authenticated) startFeed()
  })

  const views = {
    overview: Overview,
    grid: GridView,
    logs: LogsView,
  }

  let View = $derived(views[route.current.name] || Overview)
</script>

{#if !auth.checked}
  <!-- one tick while the session check runs -->
{:else if auth.required && !auth.authenticated}
  <Login />
{:else}
  <div class="shell">
    <TopBar />
    <Sidebar />
    <main>
      {#key route.current.name + route.current.query.toString()}
        <View />
      {/key}
    </main>
  </div>
{/if}

<style>
  .shell {
    display: grid;
    grid-template-areas:
      'top top'
      'side main';
    grid-template-columns: 220px 1fr;
    grid-template-rows: var(--nav-h, 48px) 1fr;
    height: 100vh;
  }
  main {
    grid-area: main;
    overflow-y: auto;
    padding: 20px;
  }
</style>
