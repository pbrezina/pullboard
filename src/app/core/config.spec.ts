import { validateConfig } from './config';
import { testConfig } from './testing';

describe('local config validation', () => {
  it('accepts the complete contract', () => expect(validateConfig(testConfig)).toEqual(testConfig));
  it('defaults the model by provider', () => {
    expect(
      validateConfig({ ...testConfig, llm: { provider: 'gemini', apiKey: 'test' } }).llm.model,
    ).toBe('gemini-1.5-flash');
    expect(
      validateConfig({ ...testConfig, llm: { provider: 'openai', apiKey: 'test' } }).llm.model,
    ).toBe('gpt-4o-mini');
  });
  it('accepts and trims an optional sprint name pattern', () => {
    expect(
      validateConfig({
        ...testConfig,
        jira: { ...testConfig.jira, sprintNamePattern: ' %YEAR-IDM-SSSD-IDENTITY-S%NUMBER ' },
      }).jira.sprintNamePattern,
    ).toBe('%YEAR-IDM-SSSD-IDENTITY-S%NUMBER');
  });
  it.each(['', '   ', null, 123, [], '%YEAR-Team-%SPRINT'])(
    'rejects invalid sprint name pattern %j',
    (sprintNamePattern) => {
      expect(() =>
        validateConfig({ ...testConfig, jira: { ...testConfig.jira, sprintNamePattern } }),
      ).toThrow('jira.sprintNamePattern');
    },
  );
  it('rejects missing tokens, HTTP, invalid project keys and unsupported LLMs', () => {
    expect(() => validateConfig({ ...testConfig, forges: [] })).toThrow();
    expect(() =>
      validateConfig({
        ...testConfig,
        forges: [{ type: 'github', apiUrl: 'https://api.github.com' }],
      }),
    ).toThrow();
    expect(() =>
      validateConfig({
        ...testConfig,
        jira: { ...testConfig.jira, baseUrl: 'http://example.com' },
      }),
    ).toThrow();
    expect(() =>
      validateConfig({ ...testConfig, jira: { ...testConfig.jira, projectKey: 'DEV OR 1=1' } }),
    ).toThrow();
    expect(() =>
      validateConfig({ ...testConfig, llm: { provider: 'other', apiKey: 'test' } }),
    ).toThrow();
  });
  it('validates discovery overrides instead of trusting arbitrary custom fields', () => {
    expect(() =>
      validateConfig({ ...testConfig, jira: { ...testConfig.jira, boardId: -1 } }),
    ).toThrow();
    expect(() =>
      validateConfig({
        ...testConfig,
        jira: { ...testConfig.jira, storyPointsFieldId: 'summary' },
      }),
    ).toThrow();
    expect(() =>
      validateConfig({ ...testConfig, jira: { ...testConfig.jira, statuses: { done: [] } } }),
    ).toThrow();
  });
});
