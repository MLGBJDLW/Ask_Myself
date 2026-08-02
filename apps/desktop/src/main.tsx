import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import { ThemeProvider } from "./lib/ThemeProvider";
import { runLocalStorageMigrations } from "./lib/localStorageMigrations";
import "@fontsource-variable/inter";
import "./index.css";
import { SpeechPlaybackProvider } from "./features/voice/SpeechPlaybackProvider";
import { OverlayProvider } from "./components/ui/overlay";

runLocalStorageMigrations();

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <ThemeProvider>
      <OverlayProvider>
        <SpeechPlaybackProvider>
          <App />
        </SpeechPlaybackProvider>
      </OverlayProvider>
    </ThemeProvider>
  </React.StrictMode>,
);
