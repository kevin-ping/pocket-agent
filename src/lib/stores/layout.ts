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

  return {
    subscribe,
    toggle,
    expand,
    collapse,
    AVATAR_W,
    AVATAR_H,
    CHAT_W,
    CHAT_H,
    EXPANDED_W,
  };
}

export const layoutStore = createLayoutStore();
