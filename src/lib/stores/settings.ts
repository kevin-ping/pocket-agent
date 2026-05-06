import { writable } from 'svelte/store';
import { invoke } from '@tauri-apps/api/core';

export interface AppSettings {
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
  fixed_lang: string;
  hotkey_code: number;
  hotkey_name: string;
  tts_enabled: boolean;
}

const defaults: AppSettings = {
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
  hotkey_code: 179,
  hotkey_name: "fn",
  tts_enabled: true,
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

    save: async (partial: Partial<AppSettings>): Promise<void> => {
      let next: AppSettings = defaults;
      update((current) => {
        next = { ...current, ...partial };
        return current; // Don't update store until save succeeds
      });
      try {
        await invoke('save_config', { config: next });
        set(next); // Only update store after successful save
      } catch (e) {
        console.error('[settings] save failed:', e);
        throw e; // Let caller handle (e.g., show error toast)
      }
    },
  };
}

export const settingsStore = createSettingsStore();
