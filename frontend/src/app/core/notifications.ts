import { Service, signal } from '@angular/core';
export interface Notice {
  id: number;
  message: string;
  kind: 'success' | 'warning' | 'error';
  url: string | null;
  label: string | null;
}

@Service()
export class Notifications {
  readonly items = signal<Notice[]>([]);
  private sequence = 0;
  show(
    message: string,
    kind: Notice['kind'] = 'success',
    url: string | null = null,
    label: string | null = null,
  ) {
    this.items.update((items) => [...items, { id: ++this.sequence, message, kind, url, label }]);
  }
  dismiss(id: number) {
    this.items.update((items) => items.filter((i) => i.id !== id));
  }
}
