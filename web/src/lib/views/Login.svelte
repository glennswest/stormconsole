<script>
  import LoginPanel from 'stormview/components/LoginPanel.svelte'
  import { nav, login } from '../stores.svelte.js'
</script>

<LoginPanel
  title={nav.name}
  subtitle="Sign in to the StormCOS console"
  onsubmit={(username, password) => login(username, password)}
/>
