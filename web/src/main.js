import { mount } from 'svelte'
import 'stormview/themes.css'
// The console's own chrome layer — loaded after the palette so its
// :root block wins on shape, elevation and type scale.
import './lib/ui/console.css'
import { initTheme } from 'stormview/theme'
import App from './App.svelte'

initTheme()

export default mount(App, { target: document.getElementById('app') })
