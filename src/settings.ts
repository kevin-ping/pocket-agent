import { mount } from 'svelte';
import SettingsApp from './SettingsApp.svelte';

const app = mount(SettingsApp, {
  target: document.getElementById('app')!,
});

export default app;
