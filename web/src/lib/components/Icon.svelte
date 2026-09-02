<script>
  // One glyph from the console icon set. Stroked, currentColor, sized in
  // px so it lines up with the text beside it.
  import { ICONS } from '../ui/icons.js'

  let { name = 'dot', size = 16, stroke = 1.6, title = null } = $props()
  const d = $derived(ICONS[name] || ICONS.dot)
</script>

<svg
  width={size}
  height={size}
  viewBox="0 0 24 24"
  fill="none"
  stroke="currentColor"
  stroke-width={stroke}
  stroke-linecap="round"
  stroke-linejoin="round"
  aria-hidden={title ? undefined : 'true'}
  role={title ? 'img' : undefined}
  style="flex-shrink:0"
>
  {#if title}<title>{title}</title>{/if}
  <path d={d} />
</svg>
