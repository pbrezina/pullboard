import { Component, inject } from '@angular/core';
import { NavigationEnd, Router } from '@angular/router';
import { toSignal } from '@angular/core/rxjs-interop';
import { filter, map } from 'rxjs';
import { Dashboard } from '../dashboard/dashboard';
import { PullRequests } from '../pull-requests/pull-requests';

@Component({
  imports: [Dashboard, PullRequests],
  selector: 'app-workspace',
  template: `
    <div [hidden]="synchronize()"><app-pull-requests /></div>
    <div [hidden]="!synchronize()"><app-dashboard /></div>
  `,
})
export class Workspace {
  private readonly router = inject(Router);
  // Both views stay mounted: navigation changes visibility, never their data lifecycle.
  protected readonly synchronize = toSignal(
    this.router.events.pipe(
      filter((event) => event instanceof NavigationEnd),
      map((event) => event.urlAfterRedirects.split(/[?#]/)[0] === '/synchronize-to-jira'),
    ),
    { initialValue: this.router.url.split(/[?#]/)[0] === '/synchronize-to-jira' },
  );
}
