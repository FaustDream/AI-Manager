import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import { AppProviders } from '@/app/providers';
import { Shell } from '@/app/shell/Shell';
import '@/index.css';

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <AppProviders>
      <Shell />
    </AppProviders>
  </StrictMode>,
);
