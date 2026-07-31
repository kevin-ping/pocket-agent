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
  avatar_gif: string | null;
  fixed_lang: string;
  ui_lang: string;
  hotkey_code: number;
  hotkey_name: string;
  tts_enabled: boolean;
  double_click_to_record: boolean;
  continuous_conversation: boolean;
  silence_timeout_secs: number;
  pause_tolerance_ms: number;
  speech_rms_threshold: number;
  barge_in_rms_threshold: number;
  barge_in_enabled: boolean;
  skip_interrupt_confirmation: boolean;
  wake_word_enabled: boolean;
  wake_word_threshold: number;
  speaker_verification_enabled: boolean;
  last_enrolled_speaker: string;
}

export const SETTINGS_DEFAULTS: AppSettings = {
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
  avatar_gif: null,
  fixed_lang: "",
  ui_lang: "en",
  hotkey_code: 60,
  hotkey_name: "RightShift",
  tts_enabled: true,
  double_click_to_record: false,
  continuous_conversation: false,
  silence_timeout_secs: 5,
  pause_tolerance_ms: 1500,
  speech_rms_threshold: 0.015,
  barge_in_rms_threshold: 0.04,
  barge_in_enabled: true,
  skip_interrupt_confirmation: true,
  wake_word_enabled: false,
  wake_word_threshold: 0.5,
  speaker_verification_enabled: false,
  last_enrolled_speaker: '',
};

function createSettingsStore() {
  const { subscribe, set, update } = writable<AppSettings>(SETTINGS_DEFAULTS);

  return {
    subscribe,
    update,

    load: async () => {
      try {
        const config = await invoke<AppSettings>('get_config');
        set({ ...SETTINGS_DEFAULTS, ...config });
        return true;
      } catch (e) {
        console.warn('[settings] load failed, using defaults:', e);
        return false;
      }
    },

    save: async (partial: Partial<AppSettings>): Promise<void> => {
      let next: AppSettings | null = null;
      // Merge partial into current store state
      update((current) => {
        next = { ...current, ...partial };
        return current; // Don't update store until save succeeds
      });
      
      // Safety check
      if (!next) {
        throw new Error('[settings] merge failed');
      }
      
      try {
        const result = await invoke<{ config: AppSettings; revision: number }>(
          'save_config',
          { config: next },
        );
        set({ ...SETTINGS_DEFAULTS, ...result.config });
      } catch (e) {
        console.error('[settings] save failed:', e);
        throw e;
      }
    },
  };
}

export const settingsStore = createSettingsStore();
