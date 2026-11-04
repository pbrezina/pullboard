import { HttpInterceptorFn } from '@angular/common/http';
import { inject, InjectionToken } from '@angular/core';
import { throwError } from 'rxjs';
import { environment } from '../../environments/environment';

/** Set by the Angular build, never by runtime settings or browser storage. */
export const TEST_MODE = new InjectionToken<boolean>('Test mode', {
  providedIn: 'root',
  factory: () => environment.testMode,
});

/** Fail closed if a new mutation path forgets to simulate its operation. */
export const testModeWrites: HttpInterceptorFn = (request, next) => {
  if (
    inject(TEST_MODE) &&
    /^\/api\/jira(?:\/|\?|$)/.test(request.url) &&
    request.method !== 'GET'
  ) {
    return throwError(() => new Error('Test mode blocks all JIRA mutation requests.'));
  }
  return next(request);
};
