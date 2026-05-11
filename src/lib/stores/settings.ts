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
  double_click_to_record: boolean;
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
  hotkey_code: 60,
  hotkey_name: "RightShift",
  tts_enabled: true,
  double_click_to_record: false,
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
      let next: AppSettings | null = null;
      let oldDoubleClickValue: boolean | undefined;
      
      // Merge partial into current store state
      update((current) => {
        oldDoubleClickValue = current.double_click_to_record;
        next = { ...current, ...partial };
        return current; // Don't update store until save succeeds
      });
      
      // Safety check
      if (!next) {
        throw new Error('[settings] merge failed');
      }
      
      try {
        // Save complete config to Rust
        await invoke('save_config', { config: next });
        
        // Update store with merged result
        set(next);
        
        
        // Notify Rust if double-click mode changed
        if (partial.double_click_to_record !== undefined && 
            oldDoubleClickValue !== partial.double_click_to_record) {
          await invoke('set_double_click_mode', { enabled: partial.double_click_to_record });
        }
      } catch (e) {
        console.error('[settings] save failed:', e);
        throw e;
      }
    },
  };
}

export const settingsStore = createSettingsStore();
