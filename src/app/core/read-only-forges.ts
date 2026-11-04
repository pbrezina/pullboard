import { HttpInterceptorFn } from '@angular/common/http';
import { throwError } from 'rxjs';

/** Defense in depth: even an accidental future provider write is rejected. */
export const readOnlyForges: HttpInterceptorFn = (request, next) => {
  if (request.url.startsWith('/api/forges/') && request.method !== 'GET') {
    return throwError(
      () => new Error('Forge APIs are read-only. Only GET requests are permitted.'),
    );
  }
  return next(request);
};
