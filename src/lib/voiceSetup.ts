// Voice setup / voice service state from the backend (src-tauri/src/voice/setup_status.rs).
// It arrives two ways — `venv-setup-*` / `voice-service-status` events and the
// `voice_setup_status` snapshot read after subscribing — each with the
// backend's sequence number. Applying only newer numbers lets the snapshot
// and events interleave without an old state overwriting a new one.

export type SetupState = 'idle' | 'installing' | 'error' | 'ready';
export type SetupSnapshot = { seq: number; state: SetupState; phase: string; detail: string };

/** Returns a function that applies a state only if it is newer than the last applied one. */
export function createSetupSequencer(apply: (s: SetupSnapshot) => void): (s: SetupSnapshot) => boolean {
  let last = 0;
  return (s) => {
    if (!(s.seq > last)) return false;
    last = s.seq;
    apply(s);
    return true;
  };
}
