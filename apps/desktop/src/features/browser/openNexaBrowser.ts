export const OPEN_BROWSER_WORKSPACE_EVENT = 'nexa:open-browser-workspace';

export interface OpenNexaBrowserDetail {
  url: string;
  title?: string;
}

/**
 * Route an HTTP(S) page to the conversation-owned Nexa Browser Workspace.
 * A mounted BrowserDock acknowledges ownership with preventDefault().
 */
export function openNexaBrowser(url: string, title?: string): boolean {
  return !window.dispatchEvent(new CustomEvent<OpenNexaBrowserDetail>(
    OPEN_BROWSER_WORKSPACE_EVENT,
    {
      detail: { url, title },
      cancelable: true,
    },
  ));
}
