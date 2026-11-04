import { Routes } from '@angular/router';

export const routes: Routes = [
  {
    path: '',
    loadComponent: () => import('./workspace/workspace').then((module) => module.Workspace),
    children: [
      { path: '', pathMatch: 'full', title: 'Pull requests · Pullboard', children: [] },
      { path: 'synchronize-to-jira', title: 'Synchronize to JIRA · Pullboard', children: [] },
    ],
  },
  { path: '**', redirectTo: '' },
];
