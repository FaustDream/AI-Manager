import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import { AppProviders } from '@/app/providers';
import { ErrorBoundary } from '@/app/shell/ErrorBoundary';
import { Shell } from '@/app/shell/Shell';
import '@/index.css';

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <ErrorBoundary>
      <AppProviders>
        <Shell />
      </AppProviders>
    </ErrorBoundary>
  </StrictMode>,
);
