import { writable, derived } from 'svelte/store';
import { invoke } from '@tauri-apps/api/core';
import { settingsStore } from './settings';
import { convLabelsLang } from '../i18n';

export type StatusTone = 'active' | 'ok' | 'warn' | 'off';

export interface StatusItem {
  id: string;
  tone: StatusTone;
  label: string;
  tooltip?: string;
  /** Bumped on each momentary event; the strip replays a one-shot pulse when it changes. */
  flash?: number;
  /** Pinned to the right edge of the strip; the others stay packed on the left. */
  end?: boolean;
}

// ─── Background subagents: count from the Rust ledger, which reads the session ───
// transcript. Lit across turns until the turn that announces the result has finished.
const backgroundSubagents = writable(0);

export function onBackgroundSubagents(count: number) {
  backgroundSubagents.set(count);
}

// At turn end: the ledger drops delegations announced (or dismissed by Hermes) this turn.
export function settleSubagents() {
  invoke('settle_background_subagents').catch((error) => console.error('settle_background_subagents failed:', error));
}

// ─── Computer tools availability (formerly the ⓘ button) ───
const computer = writable<{ available: boolean; message: string } | null>(null);

export async function refreshComputerStatus() {
  try {
    computer.set(await invoke<{ available: boolean; message: string }>('get_computer_status'));
  } catch (error) {
    computer.set({ available: false, message: String(error) });
  }
}

// ─── Speech detection: one pulse per detected speech burst ───
const speechFlash = writable(0);

export function onSpeechDetected() {
  speechFlash.update((n) => n + 1);
}

// Add new status sources here; the strip renders whatever this emits, in order (left to right).
export const statusItems = derived([backgroundSubagents, computer, speechFlash, settingsStore], ([$backgroundSubagents, $computer, $speechFlash, $settings]) => {
  const labels = convLabelsLang($settings.ui_lang);
  const items: StatusItem[] = [];
  if ($computer) {
    items.push({
      id: 'computer',
      tone: $computer.available ? 'ok' : 'off',
      label: labels.statusComputer,
      tooltip: $computer.message,
    });
  }
  // Always shown: grey when idle, yellow breathing while any background subagent is out.
  items.push({
    id: 'subagent',
    tone: $backgroundSubagents > 0 ? 'active' : 'off',
    label: labels.statusSubagent($backgroundSubagents),
  });
  items.push({
    id: 'speech',
    tone: 'off',
    label: labels.statusSpeech,
    tooltip: labels.statusSpeechHint,
    flash: $speechFlash,
    end: true,
  });
  return items;
});
