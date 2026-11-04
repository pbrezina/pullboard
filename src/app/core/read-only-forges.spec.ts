import { HttpClient, provideHttpClient, withInterceptors } from '@angular/common/http';
import { HttpTestingController, provideHttpClientTesting } from '@angular/common/http/testing';
import { TestBed } from '@angular/core/testing';
import { firstValueFrom } from 'rxjs';
import { readOnlyForges } from './read-only-forges';

describe('read-only forge boundary', () => {
  beforeEach(() =>
    TestBed.configureTestingModule({
      providers: [
        provideHttpClient(withInterceptors([readOnlyForges])),
        provideHttpClientTesting(),
      ],
    }),
  );
  afterEach(() => TestBed.inject(HttpTestingController).verify());
  it.each(['POST', 'PUT', 'PATCH', 'DELETE'])(
    'blocks %s before any network request',
    async (method) => {
      await expect(
        firstValueFrom(
          TestBed.inject(HttpClient).request(method, '/api/forges/0/repos/org/repo/pulls/42'),
        ),
      ).rejects.toThrow('read-only');
      TestBed.inject(HttpTestingController).expectNone(() => true);
    },
  );
  it('allows JIRA mutations', async () => {
    const result = firstValueFrom(
      TestBed.inject(HttpClient).post('/api/jira/rest/api/3/issue', {}),
    );
    TestBed.inject(HttpTestingController)
      .expectOne('/api/jira/rest/api/3/issue')
      .flush({ key: 'DEV-1' });
    expect(await result).toEqual({ key: 'DEV-1' });
  });
});
