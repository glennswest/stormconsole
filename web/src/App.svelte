<script>
  import { route } from './lib/router.svelte.js'
  import { auth, checkAuth, startFeed, prefs } from './lib/stores.svelte.js'
  import TopBar from './lib/components/TopBar.svelte'
  import Sidebar from './lib/components/Sidebar.svelte'
  import CreateDialog from './lib/components/CreateDialog.svelte'
  import Overview from './lib/views/Overview.svelte'
  import GridView from './lib/views/GridView.svelte'
  import LogsView from './lib/views/LogsView.svelte'
  import K8sList from './lib/views/K8sList.svelte'
  import K8sEvents from './lib/views/K8sEvents.svelte'
  import Login from './lib/views/Login.svelte'

  checkAuth().then(() => {
    if (!auth.required || auth.authenticated) startFeed()
  })

  const views = {
    overview: Overview,
    grid: GridView,
    logs: LogsView,
    k8slist: K8sList,
    k8sevents: K8sEvents,
  }

  let View = $derived(views[route.current.name] || Overview)
</script>

{#if !auth.checked}
  <!-- one tick while the session check runs -->
{:else if auth.required && !auth.authenticated}
  <Login />
{:else}
  <div class="shell" style="--sc-side: {prefs.navOpen ? 'var(--sc-nav-w)' : '0px'}">
    <TopBar />
    <Sidebar />
    <main id="main">
      {#key route.current.name + (route.current.params.kind || '') + route.current.query.toString()}
        <View />
      {/key}
    </main>
    <CreateDialog />
  </div>
{/if}

<style>
  .shell {
    display: grid;
    grid-template-areas:
      'top top'
      'side main';
    grid-template-columns: var(--sc-side) 1fr;
    grid-template-rows: var(--nav-h) 1fr;
    height: 100vh;
    background: var(--bg);
  }
  main {
    grid-area: main;
    overflow-y: auto;
    min-width: 0;
  }
</style>
