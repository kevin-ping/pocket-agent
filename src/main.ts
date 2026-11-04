import App from './App.svelte';
import MobileParked from './lib/components/mobile/MobileParked.svelte';
import { mount } from 'svelte';
import { invoke, isRemote, listen } from './lib/ipc';

// One driver at a time: while a /ui client (the phone) drives PA, the hidden
// desktop window stays an empty shell — two App instances would each answer
// every event and run every turn twice. Any switch reloads into the other mode.
// A parked phone (PA moved to the desktop) shows only a take-over button.
async function boot() {
  const target = document.getElementById('app')!;
  if (!isRemote) {
    await listen('ui-remote-active', () => location.reload());
    if (await invoke<boolean>('is_ui_remote_active')) return;
  } else {
    let parked: boolean | undefined;
    // Any change after (or racing) the question below reloads into the other mode.
    await listen<{ parked: boolean }>('ui-parked', (e) => {
      if (e.payload.parked !== parked) location.reload();
    });
    // switch_device and the camera tools need the page on screen; an Android
    // WebView in the background keeps its socket, so the page says so itself.
    const reportVisible = () => {
      invoke('set_ui_visible', { visible: document.visibilityState === 'visible' }).catch(() => {});
    };
    document.addEventListener('visibilitychange', reportVisible);
    reportVisible();
    parked = await invoke<boolean>('is_ui_parked');
    if (parked) {
      mount(MobileParked, { target });
      return;
    }
  }
  mount(App, { target });
}

void boot();
