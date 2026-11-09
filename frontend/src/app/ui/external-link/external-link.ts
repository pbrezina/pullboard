import { NgTemplateOutlet } from '@angular/common';
import { Component, computed, input } from '@angular/core';
import { safeUrl } from '../../core/board-logic';

@Component({
  selector: 'pb-link',
  template: `@if (href(); as link) {
      <a [href]="link" target="_blank" rel="noopener noreferrer" [title]="label()"
        ><ng-container [ngTemplateOutlet]="content"
      /></a>
    } @else {
      <span [title]="label()"><ng-container [ngTemplateOutlet]="content" /></span>
    }
    <ng-template #content><ng-content /></ng-template>`,
  imports: [NgTemplateOutlet],
})
export class ExternalLink {
  readonly url = input<string | null | undefined>();
  readonly label = input('');
  readonly href = computed(() => safeUrl(this.url()));
}
