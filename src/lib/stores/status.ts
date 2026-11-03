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
  /** Show the label next to the dot instead of only in the hover bubble. */
  showLabel?: boolean;
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

// ─── Hermes turn in progress: elapsed time and tool steps, ticking once a second ───
const turn = writable<{ startedAt: number; steps: number; now: number } | null>(null);
let turnTimer: ReturnType<typeof setInterval> | null = null;

/** `onTick` runs every second with the elapsed seconds (the caller's silence check). */
export function onTurnStart(onTick?: (elapsedSec: number) => void) {
  if (turnTimer) clearInterval(turnTimer);
  const startedAt = Date.now();
  turn.set({ startedAt, steps: 0, now: startedAt });
  turnTimer = setInterval(() => {
    const now = Date.now();
    turn.update((t) => (t ? { ...t, now } : t));
    onTick?.(Math.floor((now - startedAt) / 1000));
  }, 1000);
}

export function onTurnStep() {
  turn.update((t) => (t ? { ...t, steps: t.steps + 1 } : t));
}

export function onTurnEnd() {
  if (turnTimer) { clearInterval(turnTimer); turnTimer = null; }
  turn.set(null);
}

function formatElapsed(ms: number): string {
  const sec = Math.max(0, Math.floor(ms / 1000));
  return `${Math.floor(sec / 60)}:${String(sec % 60).padStart(2, '0')}`;
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
export const statusItems = derived([turn, backgroundSubagents, computer, speechFlash, settingsStore], ([$turn, $backgroundSubagents, $computer, $speechFlash, $settings]) => {
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
  if ($turn) {
    items.push({
      id: 'turn',
      tone: 'active',
      label: labels.statusTurn(formatElapsed($turn.now - $turn.startedAt), $turn.steps),
      showLabel: true,
    });
  }
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
