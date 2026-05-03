import { writable } from 'svelte/store';

export type CharacterState = 'idle' | 'listening' | 'thinking' | 'speaking';

function createCharacterStore() {
  const { subscribe, set, update } = writable<CharacterState>('idle');

  return {
    subscribe,
    toIdle: () => set('idle'),
    toListening: () => set('listening'),
    toThinking: () => set('thinking'),
    toSpeaking: () => set('speaking'),
    // 保护性跳转：只有当前状态匹配 from 时才切换，防止事件乱序导致卡死
    transition: (from: CharacterState, to: CharacterState) =>
      update((current) => {
        if (current === from) return to;
        console.warn(`[character] invalid transition ${current} → ${to} (expected from: ${from})`);
        return current;
      }),
  };
}

export const characterState = createCharacterStore();
