import React from 'react';
import ReactDOM from 'react-dom/client';
import App from './App';
import { ThemeProvider } from './lib/ThemeProvider';
import { FontProvider } from './lib/FontProvider';
import { SpeechPlaybackProvider } from './features/voice/SpeechPlaybackProvider';
import { OverlayProvider } from './components/ui/overlay';

export function mountApp() {
  ReactDOM.createRoot(document.getElementById('root') as HTMLElement).render(
    <React.StrictMode>
      <ThemeProvider>
        <FontProvider>
        <OverlayProvider>
          <SpeechPlaybackProvider>
            <App />
          </SpeechPlaybackProvider>
        </OverlayProvider>
        </FontProvider>
      </ThemeProvider>
    </React.StrictMode>,
  );
}
