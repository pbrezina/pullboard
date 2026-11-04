import { HttpErrorResponse } from '@angular/common/http';

/** Do not expose HTTP URLs, authorization headers, or provider error bodies. */
export function errorMessage(error: unknown): string {
  if (error instanceof HttpErrorResponse) {
    if (error.status === 0 || error.status === 502 || error.status === 504)
      return 'Cannot reach the API. Check your connection and regenerate the proxy, then restart ng serve.';
    if (error.status === 401 || error.status === 403)
      return 'API access denied. Check the configured token and permissions.';
    if (error.status === 404)
      return 'Resource or model not found. Check config.json and your API access.';
    if (error.status === 429) return 'API rate limit reached. Wait before trying again.';
    return `API request failed (HTTP ${error.status}). Check project fields, workflow, and permissions.`;
  }
  return error instanceof Error ? error.message : 'Operation failed. Refresh and try again.';
}
