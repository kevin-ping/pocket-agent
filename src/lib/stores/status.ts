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
}

// ─── Subagents: lifecycle only, keyed by subagent_id. Cleared at turn start. ───
const subagents = writable<Map<string, 'running' | 'done'>>(new Map());

export function onSubagentStatus(e: { subagent_id: string; phase: 'start' | 'complete' }) {
  subagents.update((m) => new Map(m).set(e.subagent_id, e.phase === 'start' ? 'running' : 'done'));
}

export function clearSubagents() {
  subagents.set(new Map());
}

// At turn end: a cancelled/failed run may never send subagent.complete, so drop
// leftovers instead of pulsing forever. A clean finish keeps its N/N.
export function settleSubagents() {
  subagents.update((m) => ([...m.values()].includes('running') ? new Map() : m));
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

// Add new status sources here; the strip renders whatever this emits, in order.
export const statusItems = derived([subagents, computer, speechFlash, settingsStore], ([$subagents, $computer, $speechFlash, $settings]) => {
  const labels = convLabelsLang($settings.ui_lang);
  const items: StatusItem[] = [];
  // Always shown: grey 0/0 when idle, yellow breathing while running, green N/N when done.
  const states = [...$subagents.values()];
  const done = states.filter((s) => s === 'done').length;
  items.push({
    id: 'subagent',
    tone: states.length === 0 ? 'off' : done < states.length ? 'active' : 'ok',
    label: labels.statusSubagent(done, states.length),
  });
  if ($computer) {
    items.push({
      id: 'computer',
      tone: $computer.available ? 'ok' : 'off',
      label: labels.statusComputer,
      tooltip: $computer.message,
    });
  }
  items.push({
    id: 'speech',
    tone: 'off',
    label: labels.statusSpeech,
    tooltip: labels.statusSpeechHint,
    flash: $speechFlash,
  });
  return items;
});
