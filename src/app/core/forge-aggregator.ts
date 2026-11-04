import { HttpClient, HttpErrorResponse } from '@angular/common/http';
import { inject, InjectionToken, Service } from '@angular/core';
import { ConfigService } from './config';
import { errorMessage } from './errors';
import { GithubProviderService } from './github-provider';
import {
  ForgeConfig,
  ForgeIdentity,
  IForgeProvider,
  PullRequestStatus,
  PullRequestInbox,
  UnifiedPullRequest,
} from './models';

export interface ForgeProviderFactory {
  type: string;
  create: (config: ForgeConfig, index: number) => IForgeProvider;
}

/** Override this token to register additional forge strategies. */
export const FORGE_PROVIDER_FACTORIES = new InjectionToken<ForgeProviderFactory[]>(
  'Forge provider factories',
  {
    providedIn: 'root',
    factory: () => {
      const http = inject(HttpClient);
      return [
        {
          type: 'github',
          create: (config, index) => new GithubProviderService(http, config, index),
        },
      ];
    },
  },
);

@Service()
export class ForgeAggregatorService {
  private readonly config = inject(ConfigService);
  private readonly factories = inject(FORGE_PROVIDER_FACTORIES);
  private providers?: Promise<IForgeProvider[]>;

  private getProviders(): Promise<IForgeProvider[]> {
    return (this.providers ??= this.config
      .load()
      .then((config) =>
        config.forges.map((forge, index) => {
          const factory = this.factories.find((entry) => entry.type === forge.type);
          if (!factory) throw new Error(`No forge provider registered for type: ${forge.type}.`);
          return factory.create(forge, index);
        }),
      )
      .catch((error: unknown) => {
        this.providers = undefined;
        throw error;
      }));
  }

  async getOpenPullRequests(): Promise<{
    pullRequests: UnifiedPullRequest[];
    identities: ForgeIdentity[];
    warnings: string[];
  }> {
    const providers = await this.getProviders();
    const config = await this.config.load();
    const results = await Promise.all(
      providers.map((provider) =>
        Promise.allSettled([provider.getUsername(), provider.getOpenPullRequests()]),
      ),
    );
    const pullRequests: UnifiedPullRequest[] = [];
    const warnings: string[] = [];
    const identities: ForgeIdentity[] = [];
    results.forEach(([identity, result], index) => {
      if (identity.status === 'fulfilled')
        identities.push({
          providerId: providers[index].id,
          label: providers[index].label,
          host: new URL(config.forges[index].webUrl ?? config.forges[index].apiUrl).host,
          username: identity.value,
        });
      else warnings.push(`${providers[index].label} account: ${errorMessage(identity.reason)}`);
      if (result.status === 'fulfilled') pullRequests.push(...result.value);
      else
        warnings.push(
          `${providers[index].label} (${providers[index].id}): ${errorMessage(result.reason)}`,
        );
    });
    return {
      pullRequests: pullRequests.sort((a, b) => b.updatedAt.localeCompare(a.updatedAt)),
      warnings,
      identities,
    };
  }

  async supportsUrl(url: string): Promise<boolean> {
    return (await this.getProviders()).some((provider) => provider.supportsUrl(url));
  }

  async getPullRequestInbox(): Promise<PullRequestInbox & { identities: ForgeIdentity[] }> {
    const providers = await this.getProviders();
    const config = await this.config.load();
    const results = await Promise.all(
      providers.map((provider) =>
        Promise.allSettled([provider.getUsername(), provider.getPullRequestInbox()]),
      ),
    );
    const inbox: PullRequestInbox & { identities: ForgeIdentity[] } = {
      opened: [],
      requested: [],
      warnings: [],
      identities: [],
    };
    results.forEach(([identity, result], index) => {
      const provider = providers[index];
      if (identity.status === 'fulfilled')
        inbox.identities.push({
          providerId: provider.id,
          label: provider.label,
          host: new URL(config.forges[index].webUrl ?? config.forges[index].apiUrl).host,
          username: identity.value,
        });
      else inbox.warnings.push(`${provider.label} account: ${errorMessage(identity.reason)}`);
      if (result.status === 'fulfilled') {
        inbox.opened.push(...result.value.opened);
        inbox.requested.push(...result.value.requested);
        inbox.warnings.push(
          ...result.value.warnings.map(
            (warning) => `${provider.label} (${provider.id}): ${warning}`,
          ),
        );
      } else
        inbox.warnings.push(
          `${provider.label} (${provider.id}): ${errorMessage(result.reason)}. Previously loaded cards for this account are not shown.`,
        );
    });
    return inbox;
  }

  async getPullRequestStatus(url: string, providerId?: string): Promise<PullRequestStatus> {
    const providers = (await this.getProviders()).filter(
      (entry) => (!providerId || entry.id === providerId) && entry.supportsUrl(url),
    );
    if (!providers.length) throw new Error('No configured forge can read this PR URL.');
    for (let index = 0; index < providers.length; index++) {
      try {
        return await providers[index].getPullRequestStatus(url);
      } catch (error) {
        // A Web Link may belong to another configured account on the same host.
        // Do not turn rate limits or connectivity errors into retries.
        if (
          !(error instanceof HttpErrorResponse) ||
          ![401, 403, 404].includes(error.status) ||
          index === providers.length - 1
        )
          throw error;
      }
    }
    throw new Error('No configured account can read this PR.');
  }
}
