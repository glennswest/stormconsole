<script>
  // An empty screen is an invitation to act: say what is not there, say
  // why in one line, and put the action that fixes it right below.
  import Icon from './Icon.svelte'

  let { icon = 'inbox', title = 'Nothing here', hint = '', action = null } = $props()
</script>

<div class="sc-empty">
  <span class="sc-empty-icon"><Icon name={icon} size={28} stroke={1.3} /></span>
  <h2>{title}</h2>
  {#if hint}<p>{hint}</p>{/if}
  {#if action}<div class="sc-empty-action">{@render action()}</div>{/if}
</div>
