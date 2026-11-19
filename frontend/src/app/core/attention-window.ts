import type { Attention } from './api-types';

export const attentionPresets = [7, 14, 30, 'all'] as const;
export const defaultAttentionDays = 14;
const maxDays = 36500;
const dayMs = 86_400_000;
export type AttentionPreset = (typeof attentionPresets)[number];
export type AttentionWindow = 'default' | AttentionPreset | `custom:${number}`;

export function parseAttentionDays(input: string): number | null {
  const days = Number(input);
  return /^\d+$/.test(input) && days >= 1 && days <= maxDays ? days : null;
}

export function parseAttentionWindow(value: unknown): AttentionWindow {
  if (value === 'all' || value === 7 || value === 14 || value === 30) return value;
  if (typeof value === 'string' && value.startsWith('custom:')) {
    const days = parseAttentionDays(value.slice(7));
    if (days !== null) return `custom:${days}`;
  }
  return 'default';
}

export function attentionCutoff(
  window: AttentionWindow,
  defaultDays: number,
  now: number,
): number | null {
  if (window === 'all') return null;
  const days =
    window === 'default'
      ? defaultDays
      : typeof window === 'number'
        ? window
        : Number(window.slice(7));
  return now - days * dayMs;
}

export function matchesAttentionWindow(
  attention: Attention | null,
  cutoff: number | null,
): boolean {
  return (
    cutoff === null ||
    !!attention?.threads.some(
      (thread) => Date.parse(thread.latest_unanswered_at ?? thread.waiting_since) >= cutoff,
    )
  );
}
