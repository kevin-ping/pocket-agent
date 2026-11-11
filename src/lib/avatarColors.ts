import { AVATAR_BG_DEFAULTS } from './stores/settings';

// Original darker gradient stops, keyed by colour, so these colours keep their look
// (including the old idle default, which saved settings may still hold).
const DEFAULT_GRAD2: Record<string, string> = {
  '#6b8cff': '#3d5af1', '#ff6b9d': '#e03068', '#ffd06b': '#e08830', '#6bffd0': '#30c888',
};

/** Mix `#rrggbb` with ~25% black. */
function darken(hex: string): string {
  const n = parseInt(hex.slice(1), 16);
  const ch = (shift: number) => Math.round(((n >> shift) & 0xff) * 0.75);
  return '#' + [16, 8, 0].map((s) => ch(s).toString(16).padStart(2, '0')).join('');
}

/** Background gradient stops for a state; falls back to the default colour. */
export function avatarGradient(state: string, color: string | undefined): { grad1: string; grad2: string } {
  const grad1 = /^#[0-9a-f]{6}$/i.test(color ?? '') ? color!.toLowerCase() : AVATAR_BG_DEFAULTS[state];
  const grad2 = DEFAULT_GRAD2[grad1] ?? darken(grad1);
  return { grad1, grad2 };
}
