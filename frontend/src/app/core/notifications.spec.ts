import { TestBed } from '@angular/core/testing';
import { Notifications } from './notifications';
describe('Notifications', () => {
  it('keeps persistent dismissible messages until acknowledged, including across newer notices', () => {
    const n = TestBed.inject(Notifications);
    for (let i = 0; i < 7; i++) n.show(`Notice ${i}`);
    expect(n.items()).toHaveLength(7);
    n.dismiss(n.items()[0].id);
    expect(n.items()).toHaveLength(6);
    expect(new Set(n.items().map((i) => i.id)).size).toBe(6);
  });
});
