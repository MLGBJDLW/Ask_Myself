import { runLocalStorageMigrations } from "./lib/localStorageMigrations";
import "@fontsource-variable/inter";
import "./index.css";
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

// Reveal the already-painted static shell independently of the React module
// graph, while loading that graph in parallel with the first native frames.
void import('./bootstrap').then(({ mountApp }) => mountApp()).catch((error: unknown) => {
  console.error('Unable to load the Nexa interface', error);
  const root = document.getElementById('root');
  if (!root) return;
  const message = document.createElement('p');
  message.setAttribute('role', 'alert');
  message.textContent = 'Unable to load Nexa.';
  const retry = document.createElement('button');
  retry.type = 'button';
  retry.textContent = 'Reload';
  retry.onclick = () => window.location.reload();
  root.replaceChildren(message, retry);
});
