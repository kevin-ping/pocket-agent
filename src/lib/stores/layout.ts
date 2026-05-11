import { writable, get } from 'svelte/store';
import { getCurrentWindow, currentMonitor, LogicalSize, LogicalPosition } from '@tauri-apps/api/window';

export type AvatarSide = 'left' | 'right';

interface LayoutState {
  expanded: boolean;
  avatarSide: AvatarSide;
  resizing: boolean;
}

const AVATAR_W = 108;
const AVATAR_H = 188;
const CHAT_W = 280;
const CHAT_H = 188;
const GAP = 12;
const EXPANDED_W = AVATAR_W + GAP + CHAT_W; // 408
const EDGE_THRESHOLD = 180; // px from right edge to flip chat side

function createLayoutStore() {
  const { subscribe, set, update } = writable<LayoutState>({
    expanded: false,
    avatarSide: 'left',
    resizing: false,
  });

  async function detectSide(): Promise<AvatarSide> {
    try {
      const win = getCurrentWindow();
      const pos = await win.outerPosition();
      const monitor = await currentMonitor();
      if (!monitor) return 'left';
      const scale = monitor.scaleFactor;
      const monitorW = monitor.workArea.size.width / scale;
      const logicalX = pos.x / scale;
      
      // Calculate if chat box would overflow on the right
      // avatarX + avatarWidth + gap + chatWidth > screenWidth => put chat on left
      const rightEdge = logicalX + AVATAR_W + GAP + CHAT_W;
      
      if (rightEdge > monitorW) {
        return 'right'; // Chat on left side
      } else {
        return 'left'; // Chat on right side (default)
      }
    } catch {
      return 'left';
    }
  }

  async function expand() {
    const win = getCurrentWindow();
    const side = await detectSide();
    update(s => ({ ...s, resizing: true, avatarSide: side }));

    try {
      const pos = await win.outerPosition();
      const monitor = await currentMonitor();
      const scale = monitor ? monitor.scaleFactor : 1;
      const logicalX = pos.x / scale;
      const logicalY = pos.y / scale;

      let newX = logicalX;
      if (side === 'right') {
        newX = logicalX - (CHAT_W + GAP);
        if (newX < 0) newX = 0;
      }

      // Shift window up so the avatar stays visually at the same screen position
      const newY = Math.max(0, logicalY - (CHAT_H - AVATAR_H) / 2);
      await win.setSize(new LogicalSize(EXPANDED_W, CHAT_H));
      await win.setPosition(new LogicalPosition(newX, newY));
    } finally {
      update(s => ({ ...s, expanded: true, resizing: false }));
    }
  }

  async function collapse() {
    const state = get({ subscribe });
    const win = getCurrentWindow();
    update(s => ({ ...s, resizing: true }));

    try {
      const pos = await win.outerPosition();
      const monitor = await currentMonitor();
      const scale = monitor ? monitor.scaleFactor : 1;
      const logicalX = pos.x / scale;
      const logicalY = pos.y / scale;

      let newX = logicalX;
      if (state.avatarSide === 'right') {
        newX = logicalX + CHAT_W + GAP;
      }

      // Restore Y: shift window back down to compensate for the upward shift during expand
      const restoredY = logicalY + (CHAT_H - AVATAR_H) / 2;
      await win.setSize(new LogicalSize(AVATAR_W, AVATAR_H));
      await win.setPosition(new LogicalPosition(newX, restoredY));
    } finally {
      update(s => ({ ...s, expanded: false, resizing: false }));
    }
  }

  async function toggle() {
    const state = get({ subscribe });
    if (state.resizing) return;
    if (state.expanded) {
      await collapse();
    } else {
      await expand();
    }
  }

  // Resize window for settings panel (called by App.svelte)
  async function openSettings(): Promise<{ x: number; y: number; w: number; h: number }> {
    const win = getCurrentWindow();
    const pos = await win.outerPosition();
    const monitor = await currentMonitor();
    const scale = monitor ? monitor.scaleFactor : 1;
    const logicalX = pos.x / scale;
    const logicalY = pos.y / scale;

    const state = get({ subscribe });
    const currentW = state.expanded ? EXPANDED_W : AVATAR_W;
    const currentH = state.expanded ? CHAT_H : AVATAR_H;

    const settingsW = 380;
    const settingsH = 500;

    // Get usable display area (respects macOS menu bar / dock)
    const workW = monitor ? monitor.workArea.size.width  / scale : 1440;
    const workH = monitor ? monitor.workArea.size.height / scale : 900;
    const workX = monitor ? monitor.workArea.position.x  / scale : 0;
    const workY = monitor ? monitor.workArea.position.y  / scale : 0;

    // Center horizontally near widget; clamp so it never goes off-screen
    let settingsX = logicalX - (settingsW - currentW) / 2;
    settingsX = Math.max(workX + 8, Math.min(settingsX, workX + workW - settingsW - 8));

    // Prefer above widget; clamp to fit within work area vertically
    let settingsY = logicalY - 60;
    settingsY = Math.max(workY + 8, Math.min(settingsY, workY + workH - settingsH - 8));

    await win.setSize(new LogicalSize(settingsW, settingsH));
    await win.setPosition(new LogicalPosition(settingsX, settingsY));

    return { x: logicalX, y: logicalY, w: currentW, h: currentH };
  }

  // Restore window after settings close
  async function closeSettings(prev: { x: number; y: number; w: number; h: number }) {
    const win = getCurrentWindow();
    await win.setSize(new LogicalSize(prev.w, prev.h));
    await win.setPosition(new LogicalPosition(prev.x, prev.y));
  }

  return {
    subscribe,
    toggle,
    expand,
    collapse,
    openSettings,
    closeSettings,
    AVATAR_W,
    AVATAR_H,
    CHAT_W,
    CHAT_H,
    EXPANDED_W,
  };
}

export const layoutStore = createLayoutStore();
