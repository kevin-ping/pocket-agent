// ipc.ts — invoke / listen for both shells (FEAT-DEV2).
//
// In the Tauri window this is @tauri-apps/api. In a plain browser — the
// phone's WebView loading http://<mac>:8650/ui/?token=… — it rides the
// /ui/ws socket instead: commands go out as {id, cmd, args}, events come back
// as {type:"event"}. The key is read from the page URL, never built in.
//
// Remote mode has no window: callers must not touch @tauri-apps/api/window
// when `isRemote` is true.

import { invoke as tauriInvoke } from '@tauri-apps/api/core';
import { listen as tauriListen, type Event, type EventCallback, type UnlistenFn } from '@tauri-apps/api/event';

export type { UnlistenFn };

export const isRemote = typeof window !== 'undefined' && !('__TAURI_INTERNALS__' in window);

/** Server close code for a page replaced by a newer /ui client (api/remote.rs). */
const CLOSE_REPLACED = 4001;
const RETRY_MS = 3000;

type Pending = { resolve: (value: unknown) => void; reject: (reason: unknown) => void };

const handlers = new Map<string, Set<EventCallback<unknown>>>();
const pending = new Map<number, Pending>();
let nextId = 0;
let socket: Promise<WebSocket> | null = null;

function socketUrl(): string {
  const token = new URLSearchParams(location.search).get('token') ?? '';
  const scheme = location.protocol === 'https:' ? 'wss' : 'ws';
  return `${scheme}://${location.host}/ui/ws?token=${encodeURIComponent(token)}`;
}

function showNotice(text: string) {
  let el = document.getElementById('ipc-notice');
  if (!el) {
    el = document.createElement('div');
    el.id = 'ipc-notice';
    el.style.cssText = 'position:fixed;left:0;right:0;bottom:0;padding:10px;text-align:center;'
      + 'font:14px system-ui;color:#fff;background:rgba(0,0,0,.75);z-index:9999';
    document.body.appendChild(el);
  }
  el.textContent = text;
}

/** Wait until the Mac answers again, then reload: the page's state missed events while away. */
async function reloadWhenReachable() {
  showNotice('与 Pocket Agent 的连接已断开，正在重连…');
  for (;;) {
    try {
      const res = await fetch(location.href, { cache: 'no-store' });
      if (res.ok) { location.reload(); return; }
    } catch { /* still unreachable */ }
    await new Promise((r) => setTimeout(r, RETRY_MS));
  }
}

function onMessage(data: unknown) {
  if (typeof data !== 'string') return;
  let msg: { type?: string; event?: string; payload?: unknown; id?: number; ok?: unknown; err?: unknown };
  try { msg = JSON.parse(data); } catch { return; }
  if (msg.type === 'event' && msg.event) {
    const event = { event: msg.event, id: 0, payload: msg.payload } as Event<unknown>;
    handlers.get(msg.event)?.forEach((h) => h(event));
  } else if (msg.type === 'result' && typeof msg.id === 'number') {
    const p = pending.get(msg.id);
    if (!p) return;
    pending.delete(msg.id);
    if ('err' in msg) p.reject(msg.err);
    else p.resolve(msg.ok);
  }
}

function connect(): Promise<WebSocket> {
  if (socket) return socket;
  socket = new Promise((resolve) => {
    const ws = new WebSocket(socketUrl());
    let opened = false;
    ws.onopen = () => { opened = true; document.getElementById('ipc-notice')?.remove(); resolve(ws); };
    ws.onmessage = (m) => onMessage(m.data);
    ws.onclose = (e) => {
      for (const p of pending.values()) p.reject('connection closed');
      pending.clear();
      if (e.code === CLOSE_REPLACED) {
        showNotice('Pocket Agent 已在另一台设备上打开');
        return; // reconnecting would kick the newer page right back
      }
      if (opened) { void reloadWhenReachable(); return; }
      // Never opened: Mac unreachable or the key was refused (401 looks the same here).
      showNotice('无法连接 Pocket Agent(检查地址和 key),稍后重试…');
      socket = null;
      setTimeout(() => { void connect().then(resolve); }, RETRY_MS);
    };
  });
  return socket;
}

export async function invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  if (!isRemote) return tauriInvoke<T>(cmd, args);
  const ws = await connect();
  if (ws.readyState !== WebSocket.OPEN) throw 'connection closed';
  const id = ++nextId;
  return new Promise<T>((resolve, reject) => {
    pending.set(id, { resolve: resolve as (value: unknown) => void, reject });
    ws.send(JSON.stringify({ id, cmd, args: args ?? {} }));
  });
}

export async function listen<T>(event: string, handler: EventCallback<T>): Promise<UnlistenFn> {
  if (!isRemote) return tauriListen<T>(event, handler);
  let set = handlers.get(event);
  if (!set) { set = new Set(); handlers.set(event, set); }
  const h = handler as EventCallback<unknown>;
  set.add(h);
  void connect();
  return () => { set!.delete(h); };
}
