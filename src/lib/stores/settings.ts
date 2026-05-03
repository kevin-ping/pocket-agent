import { writable } from 'svelte/store';
import { invoke } from '@tauri-apps/api/core';

export interface AppSettings {
  api_url: string;
  volume: number;
  character_skin: 'default-css' | 'rive' | 'lottie';
  dialog_style: 'bubble' | 'tv' | 'terminal';
  tts_format: 'wav' | 'mp3';
  tts_primary_voice: string;
  tts_aux1_voice: string;
  tts_aux2_voice: string;
  window_x: number | null;
  window_y: number | null;
  avatar_image: string | null;
  fixed_lang: string;  // "", "primary", "aux1", "aux2"
}

const defaults: AppSettings = {
  api_url: 'http://localhost:8642',
  volume: 0.8,
  character_skin: 'default-css',
  dialog_style: 'bubble',
  tts_format: 'wav',
  tts_primary_voice: 'zh-CN-XiaoxiaoNeural',
  tts_aux1_voice: '',
  tts_aux2_voice: '',
  window_x: null,
  window_y: null,
  avatar_image: null,
  fixed_lang: "",
};

function createSettingsStore() {
  const { subscribe, set, update } = writable<AppSettings>(defaults);

  return {
    subscribe,
    update,

    load: async () => {
      try {
        const config = await invoke<AppSettings>('get_config');
        set({ ...defaults, ...config });
      } catch (e) {
        console.warn('[settings] load failed, using defaults:', e);
      }
    },

    save: async (partial: Partial<AppSettings>) => {
      update((s) => {
        const next = { ...s, ...partial };
        invoke('save_config', { config: next }).catch((e) =>
          console.error('[settings] save failed:', e)
        );
        return next;
      });
    },
  };
}

export const settingsStore = createSettingsStore();
