import { provideHttpClient } from '@angular/common/http';
import { HttpTestingController, provideHttpClientTesting } from '@angular/common/http/testing';
import { TestBed } from '@angular/core/testing';
import { ConfigService } from './config';
import { LlmService, parseSuggestion } from './llm';
import { testConfig, testPr } from './testing';

const suggestion = {
  title: 'Fix shutdown',
  description: 'Wait for workers. Verify graceful shutdown.',
  storyPoints: 3,
};

describe('LLM suggestions', () => {
  it('validates JSON, allowing fenced responses', () => {
    expect(parseSuggestion(`\`\`\`json\n${JSON.stringify(suggestion)}\n\`\`\``)).toEqual(
      suggestion,
    );
  });
  it.each([
    'not json',
    '{}',
    JSON.stringify({ ...suggestion, storyPoints: -1 }),
    JSON.stringify({ ...suggestion, title: 'x'.repeat(256) }),
    JSON.stringify({ ...suggestion, storyPoints: '3' }),
  ])('rejects invalid model output: %s', (value) => {
    expect(() => parseSuggestion(value)).toThrow();
  });

  for (const provider of ['openai', 'gemini'] as const) {
    it(`requests review-specific scope and effort through ${provider}`, async () => {
      TestBed.configureTestingModule({
        providers: [
          provideHttpClient(),
          provideHttpClientTesting(),
          {
            provide: ConfigService,
            useValue: {
              load: async () => ({ ...testConfig, llm: { ...testConfig.llm, provider } }),
            },
          },
        ],
      });
      const pending = TestBed.inject(LlmService).suggest(testPr, true);
      await Promise.resolve();
      const http = TestBed.inject(HttpTestingController);
      const request = http.expectOne((request) => request.url.startsWith(`/api/${provider}/`));
      const body = JSON.stringify(request.request.body);
      expect(body).toContain('not implementing');
      expect(body).toContain('Estimate only the review effort');
      expect(body).toContain('untrusted data');
      expect(body).toContain(testPr.repository);
      expect(body).toContain(testPr.url);
      expect(body).not.toContain('test-only');
      request.flush(
        provider === 'openai'
          ? { choices: [{ message: { content: JSON.stringify(suggestion) } }] }
          : { candidates: [{ content: { parts: [{ text: JSON.stringify(suggestion) }] } }] },
      );
      expect(await pending).toEqual(suggestion);
      http.verify();
    });

    it(`uses the ${provider} proxy with header authentication and no config secrets in the prompt`, async () => {
      TestBed.configureTestingModule({
        providers: [
          provideHttpClient(),
          provideHttpClientTesting(),
          {
            provide: ConfigService,
            useValue: {
              load: async () => ({
                ...testConfig,
                llm: {
                  ...testConfig.llm,
                  provider,
                  model: provider === 'gemini' ? 'gemini-1.5-flash' : 'gpt-4o-mini',
                },
              }),
            },
          },
        ],
      });
      const pending = TestBed.inject(LlmService).suggest(testPr);
      await Promise.resolve();
      const http = TestBed.inject(HttpTestingController);
      const request = http.expectOne((value) => value.url.startsWith(`/api/${provider}/`));
      expect(request.request.method).toBe('POST');
      expect(request.request.url).not.toContain('test-only');
      expect(JSON.stringify(request.request.body)).not.toContain('test-only');
      expect(JSON.stringify(request.request.body)).toContain(testPr.title);
      if (provider === 'openai') {
        expect(request.request.headers.get('Authorization')).toBe('Bearer test-only');
        request.flush({ choices: [{ message: { content: JSON.stringify(suggestion) } }] });
      } else {
        expect(request.request.headers.get('x-goog-api-key')).toBe('test-only');
        request.flush({
          candidates: [{ content: { parts: [{ text: JSON.stringify(suggestion) }] } }],
        });
      }
      expect(await pending).toEqual(suggestion);
      http.verify();
    });
  }
});
