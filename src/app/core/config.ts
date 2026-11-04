import { HttpClient } from '@angular/common/http';
import { inject, Service } from '@angular/core';
import { firstValueFrom } from 'rxjs';
import { AppConfig } from './models';
import { compileSprintNamePattern } from './sprint-name-pattern';

function object(value: unknown): Record<string, unknown> {
  if (!value || typeof value !== 'object' || Array.isArray(value))
    throw new Error('Invalid config.json object.');
  return value as Record<string, unknown>;
}
function text(value: unknown, field: string): string {
  if (typeof value !== 'string' || !value.trim()) throw new Error(`Set ${field} in config.json.`);
  return value.trim();
}
function https(value: unknown, field: string): string {
  const result = text(value, field);
  const url = new URL(result);
  if (url.protocol !== 'https:' || url.username || url.password || url.search || url.hash) {
    throw new Error(`${field} must be an HTTPS URL without credentials, query, or fragment.`);
  }
  return result.replace(/\/$/, '');
}

export function validateConfig(raw: unknown): AppConfig {
  const config = object(raw);
  if (!Array.isArray(config['forges']) || !config['forges'].length)
    throw new Error('Configure at least one forge.');
  const forges = config['forges'].map((entry: unknown) => {
    const forge = object(entry);
    return {
      type: text(forge['type'], 'forges.type'),
      token: text(forge['token'], 'forges.token'),
      apiUrl: https(forge['apiUrl'], 'forges.apiUrl'),
      ...(forge['webUrl'] ? { webUrl: https(forge['webUrl'], 'forges.webUrl') } : {}),
    };
  });
  const jira = object(config['jira']);
  const llm = object(config['llm']);
  if (llm['provider'] !== 'gemini' && llm['provider'] !== 'openai')
    throw new Error('llm.provider must be gemini or openai.');
  const projectKey = text(jira['projectKey'], 'jira.projectKey');
  if (!/^[A-Z][A-Z0-9_]*$/.test(projectKey))
    throw new Error('jira.projectKey must be a JIRA project key.');
  const optional: Pick<
    AppConfig['jira'],
    'boardId' | 'sprintId' | 'sprintNamePattern' | 'issueTypeId' | 'storyPointsFieldId' | 'statuses'
  > = {};
  for (const field of ['boardId', 'sprintId'] as const) {
    if (jira[field] !== undefined) {
      if (!Number.isSafeInteger(jira[field]) || Number(jira[field]) < 1)
        throw new Error(`jira.${field} must be a positive integer.`);
      optional[field] = Number(jira[field]);
    }
  }
  for (const field of ['sprintNamePattern', 'issueTypeId', 'storyPointsFieldId'] as const) {
    if (jira[field] !== undefined) optional[field] = text(jira[field], `jira.${field}`);
  }
  if (optional.sprintNamePattern) compileSprintNamePattern(optional.sprintNamePattern);
  if (optional.storyPointsFieldId && !/^customfield_\d+$/.test(optional.storyPointsFieldId))
    throw new Error('Invalid storyPointsFieldId.');
  if (jira['statuses'] !== undefined) {
    const statuses = object(jira['statuses']);
    optional.statuses = {};
    for (const field of ['inProgress', 'review', 'done'] as const) {
      if (statuses[field] !== undefined) {
        if (!Array.isArray(statuses[field]) || !statuses[field].length)
          throw new Error(`jira.statuses.${field} must be a nonempty array.`);
        optional.statuses[field] = statuses[field].map((entry: unknown) =>
          text(entry, `jira.statuses.${field}`),
        );
      }
    }
  }
  return {
    forges,
    jira: {
      ...optional,
      email: text(jira['email'], 'jira.email'),
      apiToken: text(jira['apiToken'], 'jira.apiToken'),
      baseUrl: https(jira['baseUrl'], 'jira.baseUrl'),
      projectKey,
    },
    llm: {
      provider: llm['provider'],
      apiKey: text(llm['apiKey'], 'llm.apiKey'),
      model: llm['model']
        ? text(llm['model'], 'llm.model')
        : llm['provider'] === 'gemini'
          ? 'gemini-1.5-flash'
          : 'gpt-4o-mini',
    },
  };
}

@Service()
export class ConfigService {
  private readonly http = inject(HttpClient);
  private pending?: Promise<AppConfig>;

  load(): Promise<AppConfig> {
    return (this.pending ??= firstValueFrom(
      this.http.get<unknown>('/assets/config.json', {
        headers: { 'Cache-Control': 'no-cache' },
      }),
    )
      .then(validateConfig)
      .catch((error: unknown) => {
        this.pending = undefined;
        throw error;
      }));
  }
}
