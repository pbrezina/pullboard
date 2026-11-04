import { ComponentFixture, TestBed } from '@angular/core/testing';
import { PrActivity } from './pr-activity';
import { testPr } from '../core/testing';

describe('PR inline activity metadata', () => {
  let fixture: ComponentFixture<PrActivity>;
  beforeEach(async () => {
    await TestBed.configureTestingModule({ imports: [PrActivity] }).compileComponents();
    fixture = TestBed.createComponent(PrActivity);
    fixture.componentRef.setInput('pr', {
      ...testPr,
      createdAt: '2026-04-01T10:00:00Z',
      updatedAt: '2026-06-02T10:00:00Z',
    });
    fixture.componentRef.setInput('asOf', Date.parse('2026-11-04T10:00:00Z'));
  });
  it('shows creation date and spelled-out ages with only the values bold', async () => {
    fixture.componentRef.setInput('waitingSince', '2026-06-01T10:00:00Z');
    await fixture.whenStable();
    const element: HTMLElement = fixture.nativeElement;
    expect(element.textContent?.replace(/\s+/g, ' ').replace(/\s+,/g, ',').trim()).toBe(
      'Created: 2026-04-01, Last activity: 155 days, Waiting since: 156 days',
    );
    expect(
      [...element.querySelectorAll('strong')].map((value) => value.textContent?.trim()),
    ).toEqual(['2026-04-01', '155 days', '156 days']);
    expect(element.querySelector('.last-activity time')?.getAttribute('datetime')).toBe(
      '2026-06-02T10:00:00Z',
    );
    expect(element.querySelector('.created time')?.getAttribute('title')).toBeTruthy();
  });
  it('omits waiting metadata when there is nothing pending', async () => {
    await fixture.whenStable();
    expect(fixture.nativeElement.querySelector('.waiting-since')).toBeNull();
  });
  it('marks missing dates as unknown without inventing a timestamp', async () => {
    fixture.componentRef.setInput('pr', testPr);
    fixture.componentRef.setInput('requestTimeUnknown', true);
    await fixture.whenStable();
    expect(fixture.nativeElement.querySelector('.created strong').textContent.trim()).toBe(
      'Unknown',
    );
    expect(fixture.nativeElement.querySelector('.waiting-since strong').textContent.trim()).toBe(
      'Unknown',
    );
    expect(fixture.nativeElement.querySelector('.created time')).toBeNull();
  });
});
