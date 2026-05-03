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
  friendly:  80,
  cheerful:  65,
  calm:     120,
  serious:  100,
  sad:      160,
  whisper:  140,
  excited:  45,
  angry:     55,
};

let typewriterTimer: ReturnType<typeof setInterval> | null = null;
let pendingChars: string[] = [];
let currentSpeed: number = EMOTION_SPEEDS.friendly;
let streamEnding = false;

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
    const batch = currentSpeed < 50 ? 2 : 1;
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
      let chunk = '';
      for (let i = 0; i < batch && pendingChars.length > 0; i++) {
        chunk += pendingChars.shift();
      }
      update((s) => ({ ...s, streamingContent: s.streamingContent + chunk }));
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

    startStream: () =>
      update((s) => ({ ...s, streamingContent: '', isStreaming: true, error: null })),

    startTypewriter,

    appendDelta: (delta: string) => {
      for (const ch of delta) {
        pendingChars.push(ch);
      }
      // If timer isn't running but we have chars, start it
      if (!typewriterTimer && pendingChars.length > 0) {
        const batch = currentSpeed < 50 ? 2 : 1;
        typewriterTimer = setInterval(() => {
          if (pendingChars.length === 0) {
            clearInterval(typewriterTimer!);
            typewriterTimer = null;
            if (streamEnding) {
              finalizeStream();
            }
            return;
          }
          let chunk = '';
          for (let i = 0; i < batch && pendingChars.length > 0; i++) {
            chunk += pendingChars.shift();
          }
          update((s) => ({ ...s, streamingContent: s.streamingContent + chunk }));
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
