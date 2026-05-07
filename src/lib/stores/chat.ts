import { writable } from 'svelte/store';

export interface ChatMessage {
  role: 'user' | 'assistant';
  content: string;
  timestamp: number;
}

interface ChatState {
  messages: ChatMessage[];
  streamingContent: string;
  isStreaming: boolean;
  error: string | null;
  /** Steps shown during LLM thinking/tool-calling phase (cleared when final text arrives) */
  thinkingSteps: string[];
}

// ── Emotion → ms per char ──
const EMOTION_SPEEDS: Record<string, number> = {
  friendly:  210,
  cheerful:  192,
  calm:  220,
  serious:  232,
  sad:  276,
  whisper:  259,
  excited:  192,
  angry:  184,
};

// ── Script detection ──
type ScriptType = 'cjk' | 'word-based' | 'mixed';

const SCRIPT_MULTIPLIERS: Record<ScriptType, number> = {
  'cjk':        1.0,
  'word-based': 2.0,
  'mixed':      1.5,
};

function detectScript(text: string): ScriptType {
  let cjk = 0, alpha = 0;
  for (const ch of text) {
    const code = ch.codePointAt(0)!;
    if ((code >= 0x4E00 && code <= 0x9FFF) ||
        (code >= 0x3040 && code <= 0x30FF) ||
        (code >= 0xAC00 && code <= 0xD7AF)) cjk++;
    else if ((code >= 0x41 && code <= 0x5A) ||
             (code >= 0x61 && code <= 0x7A) ||
             (code >= 0x0400 && code <= 0x04FF) ||
             (code >= 0x0600 && code <= 0x06FF) ||
             (code >= 0x0370 && code <= 0x03FF)) alpha++;
  }
  const total = cjk + alpha;
  if (total === 0) return 'cjk';
  if (cjk / total > 0.6) return 'cjk';
  if (alpha / total > 0.5) return 'word-based';
  return 'mixed';
}

let typewriterTimer: ReturnType<typeof setInterval> | null = null;
let pendingChars: string[] = [];
let currentSpeed: number = EMOTION_SPEEDS.friendly;
let currentEmotion: string = 'friendly';
let scriptDetected: boolean = false;
let streamEnding = false;
// hold refs to store functions so timer callbacks can use them
let storeUpdate: any;
let storeFinalize: (() => void) | null = null;

function pullNextUnit(chars: string[]): string {
  if (chars.length === 0) return '';
  const first = chars[0];
  if (first.charCodeAt(0) > 0x2000) {
    return chars.shift()!;
  }
  let unit = '';
  let gotLetter = false;
  while (chars.length > 0) {
    const ch = chars[0];
    const code = ch.charCodeAt(0);
    if (code > 0x2000) break;
    if (ch === ' ' && gotLetter) {
      unit += chars.shift()!;
      break;
    }
    if (/[a-zA-Z0-9]/.test(ch)) gotLetter = true;
    unit += chars.shift()!;
    if (gotLetter && ch === ' ') break;
  }
  return unit;
}

function startTimer() {
  if (typewriterTimer) clearInterval(typewriterTimer);
  if (pendingChars.length === 0) { typewriterTimer = null; return; }

  typewriterTimer = setInterval(() => {
    if (pendingChars.length === 0) {
      clearInterval(typewriterTimer!);
      typewriterTimer = null;
      if (streamEnding && storeFinalize) storeFinalize();
      return;
    }
    const chunk = pullNextUnit(pendingChars);
    if (chunk && storeUpdate) storeUpdate((s: any) => ({ ...s, streamingContent: s.streamingContent + chunk }));
  }, currentSpeed);
}

function createChatStore() {
  const { subscribe, update, set } = writable<ChatState>({
    messages: [],
    streamingContent: '',
    isStreaming: false,
    error: null,
    thinkingSteps: [],
  });

  storeUpdate = update;

  function finalizeStream() {
    update((s) => {
      if (!s.streamingContent) return { ...s, isStreaming: false };
      return {
        messages: [
          ...s.messages,
          { role: 'assistant', content: s.streamingContent, timestamp: Date.now() },
        ],
        streamingContent: '',
        isStreaming: false,
        error: null,
        thinkingSteps: [],
      };
    });
  }

  storeFinalize = finalizeStream;

  return {
    subscribe,

    addUserMessage: (content: string) =>
      update((s) => ({
        ...s,
        error: null,
        messages: [...s.messages, { role: 'user', content, timestamp: Date.now() }],
      })),

    addBotMessage: (content: string) =>
      update((s) => ({
        ...s,
        error: null,
        messages: [...s.messages, { role: 'assistant', content, timestamp: Date.now() }],
      })),

    startStream: () =>
      update((s) => ({ ...s, streamingContent: '', isStreaming: true, error: null, thinkingSteps: [] })),

    /** Add an intermediate step shown during LLM thinking/tool-calling phase */
    addThinkingStep: (step: string) =>
      update((s) => {
        // Avoid duplicating the same consecutive step
        if (s.thinkingSteps.length > 0 && s.thinkingSteps[s.thinkingSteps.length - 1] === step) {
          return s;
        }
        return { ...s, thinkingSteps: [...s.thinkingSteps, step] };
      }),

    /** Update the last thinking step in-place (reasoning comes as many tiny chunks) */
    updateLastThinkingStep: (text: string) =>
      update((s) => {
        if (s.thinkingSteps.length === 0) {
          return { ...s, thinkingSteps: [`🤔 ${text}`] };
        }
        const last = s.thinkingSteps[s.thinkingSteps.length - 1];
        if (last.startsWith('🤔')) {
          const updated = [...s.thinkingSteps];
          updated[updated.length - 1] = `🤔 ${text}`;
          return { ...s, thinkingSteps: updated };
        }
        return { ...s, thinkingSteps: [...s.thinkingSteps, `🤔 ${text}`] };
      }),

    /** Clear all thinking steps — called when final text arrives */
    clearThinkingSteps: () =>
      update((s) => ({ ...s, thinkingSteps: [] })),

    startTypewriter: (emotion: string) => {
      if (typewriterTimer) {
        clearInterval(typewriterTimer);
        typewriterTimer = null;
      }
      if (pendingChars.length > 0) {
        const remaining = pendingChars.join('');
        pendingChars = [];
        update((s) => ({ ...s, streamingContent: s.streamingContent + remaining }));
      }
      streamEnding = false;
      scriptDetected = false;
      currentEmotion = emotion;
      currentSpeed = EMOTION_SPEEDS[emotion] ?? EMOTION_SPEEDS.friendly;
    },

    appendDelta: (delta: string) => {
      for (const ch of delta) pendingChars.push(ch);

      // Detect script once we have enough text
      if (!scriptDetected && pendingChars.length >= 8) {
        const script = detectScript(pendingChars.join(''));
        const multiplier = SCRIPT_MULTIPLIERS[script];
        const newSpeed = (EMOTION_SPEEDS[currentEmotion] ?? EMOTION_SPEEDS.friendly) * multiplier;
        if (newSpeed !== currentSpeed) {
          currentSpeed = newSpeed;
          if (typewriterTimer) {
            clearInterval(typewriterTimer);
            typewriterTimer = null;
            if (pendingChars.length > 0) startTimer();
          }
        }
        scriptDetected = true;
      }

      if (!typewriterTimer && pendingChars.length > 0) {
        startTimer();
      }
    },

    endStream: () => {
      streamEnding = true;
      if (!typewriterTimer) finalizeStream();
    },

    setError: (msg: string) => {
      if (typewriterTimer) { clearInterval(typewriterTimer); typewriterTimer = null; }
      if (pendingChars.length > 0) pendingChars = [];
      streamEnding = false;
      update((s) => ({ ...s, isStreaming: false, streamingContent: '', error: msg, thinkingSteps: [] }));
    },

    clear: () => {
      if (typewriterTimer) { clearInterval(typewriterTimer); typewriterTimer = null; }
      if (pendingChars.length > 0) pendingChars = [];
      streamEnding = false;
      set({ messages: [], streamingContent: '', isStreaming: false, error: null, thinkingSteps: [] });
    },
  };
}

export const chatStore = createChatStore();
export { EMOTION_SPEEDS };
