import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import { ThemeProvider } from "./lib/ThemeProvider";
import { runLocalStorageMigrations } from "./lib/localStorageMigrations";
import "@fontsource-variable/inter";
import "./index.css";
import { SpeechPlaybackProvider } from "./features/voice/SpeechPlaybackProvider";
import { OverlayProvider } from "./components/ui/overlay";
import { getCurrentWindow } from "@tauri-apps/api/window";

function revealMainWindowAfterFirstPaint() {
  if (!("__TAURI_INTERNALS__" in window)) return;
  let appWindow: ReturnType<typeof getCurrentWindow>;
  try {
    appWindow = getCurrentWindow();
  } catch {
    // Browser previews and partial test shims do not have native window metadata.
    return;
  }
  if (appWindow.label !== "main") return;
  requestAnimationFrame(() => {
    requestAnimationFrame(() => {
      void (async () => {
        await appWindow.show();
        await appWindow.setFocus();
      })().catch((error: unknown) => {
        console.warn("Unable to reveal the main window after startup paint", error);
      });
    });
  });
}

revealMainWindowAfterFirstPaint();

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
