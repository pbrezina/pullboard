import { DestroyRef, inject, Service, signal } from '@angular/core';

@Service()
export class Clock {
  readonly now = signal(Date.now());
  constructor() {
    const timer = setInterval(() => this.now.set(Date.now()), 30_000);
    inject(DestroyRef).onDestroy(() => clearInterval(timer));
  }
}
export function elapsed(timestamp: string | null, now: number): string {
  if (!timestamp) return 'Not yet';
  const parsed = Date.parse(timestamp);
  if (!Number.isFinite(parsed)) return 'Unknown';
  const seconds = Math.max(0, Math.floor((now - parsed) / 1000));
  if (seconds < 60) return 'Just now';
  if (seconds < 3600) return `${Math.floor(seconds / 60)}m ago`;
  if (seconds < 86400) return `${Math.floor(seconds / 3600)}h ago`;
  return `${Math.floor(seconds / 86400)}d ago`;
}
