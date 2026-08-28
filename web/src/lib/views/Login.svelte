<script>
  import LoginPanel from 'stormview/components/LoginPanel.svelte'
  import { nav, login } from '../stores.svelte.js'
</script>

<LoginPanel
  title={nav.name}
  subtitle="stormconsole · sign in"
  onsubmit={(username, password) => login(username, password)}
/>
