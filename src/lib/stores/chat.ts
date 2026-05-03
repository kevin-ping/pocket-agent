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
}

// ── Emotion → ms per char ──
const EMOTION_SPEEDS: Record<string, number> = {
  friendly:  210,
  cheerful:  192,
  calm:  245,
  serious:  232,
  sad:  276,
  whisper:  259,
  excited:  192,
  angry:  184,
};

let typewriterTimer: ReturnType<typeof setInterval> | null = null;
let pendingChars: string[] = [];
let currentSpeed: number = EMOTION_SPEEDS.friendly;
let streamEnding = false;

// Pull next display unit: one CJK/punct char, or one continuous ASCII word (including trailing space)
function pullNextUnit(chars: string[]): string {
  if (chars.length === 0) return '';
  const first = chars[0];
  // CJK or fullwidth or non-ASCII punctuation: single char
  if (first.charCodeAt(0) > 0x2000) {
    return chars.shift()!;
  }
  // ASCII: pull until we hit non-ASCII or space-after-word
  let unit = '';
  let gotLetter = false;
  while (chars.length > 0) {
    const ch = chars[0];
    const code = ch.charCodeAt(0);
    if (code > 0x2000) break; // hit CJK
    if (ch === ' ' && gotLetter) {
      unit += chars.shift()!;
      break; // include trailing space, then stop
    }
    if (/[a-zA-Z0-9]/.test(ch)) gotLetter = true;
    unit += chars.shift()!;
    // stop at space if we've been collecting letters
    if (gotLetter && ch === ' ') break;
  }
  return unit;
}

function createChatStore() {
  const { subscribe, update, set } = writable<ChatState>({
    messages: [],
    streamingContent: '',
    isStreaming: false,
    error: null,
  });

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
      };
    });
  }

  function startTypewriter(emotion: string) {
    stopTypewriter();
    streamEnding = false;
    currentSpeed = EMOTION_SPEEDS[emotion] ?? EMOTION_SPEEDS.friendly;
    typewriterTimer = setInterval(() => {
      if (pendingChars.length === 0) {
        clearInterval(typewriterTimer!);
        typewriterTimer = null;
        // Typewriter finished naturally
        if (streamEnding) {
          finalizeStream();
        }
        return;
      }
      const chunk = pullNextUnit(pendingChars);
      if (chunk) update((s) => ({ ...s, streamingContent: s.streamingContent + chunk }));
    }, currentSpeed);
  }

  function stopTypewriter() {
    if (typewriterTimer) {
      clearInterval(typewriterTimer);
      typewriterTimer = null;
    }
    // Flush remaining
    if (pendingChars.length > 0) {
      const remaining = pendingChars.join('');
      pendingChars = [];
      update((s) => ({ ...s, streamingContent: s.streamingContent + remaining }));
    }
  }

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
      update((s) => ({ ...s, streamingContent: '', isStreaming: true, error: null })),

    startTypewriter,

    appendDelta: (delta: string) => {
      for (const ch of delta) {
        pendingChars.push(ch);
      }
      // If timer isn't running but we have chars, start it
      if (!typewriterTimer && pendingChars.length > 0) {
            typewriterTimer = setInterval(() => {
          if (pendingChars.length === 0) {
            clearInterval(typewriterTimer!);
            typewriterTimer = null;
            if (streamEnding) {
              finalizeStream();
            }
            return;
          }
          const chunk = pullNextUnit(pendingChars);
          if (chunk) update((s) => ({ ...s, streamingContent: s.streamingContent + chunk }));
        }, currentSpeed);
      }
    },

    // Mark stream as ending — typewriter will finalize when it drains
    endStream: () => {
      streamEnding = true;
      if (!typewriterTimer) {
        // Timer already stopped, finalize now
        finalizeStream();
      }
      // else: timer is still running, let it drain and finalize when done
    },

    setError: (msg: string) => {
      stopTypewriter();
      streamEnding = false;
      update((s) => ({ ...s, isStreaming: false, streamingContent: '', error: msg }));
    },

    clear: () => {
      stopTypewriter();
      streamEnding = false;
      set({ messages: [], streamingContent: '', isStreaming: false, error: null });
    },
  };
}

export const chatStore = createChatStore();
export { EMOTION_SPEEDS };
