import {
  createContext,
  useContext,
  useState,
  type ReactNode,
} from 'react';

const OverlayRootContext = createContext<HTMLElement | null>(null);

export function OverlayProvider({ children }: { children: ReactNode }) {
  const [root, setRoot] = useState<HTMLDivElement | null>(null);

  return (
    <OverlayRootContext.Provider value={root}>
      {children}
      <div
        ref={setRoot}
        id="nexa-overlay-root"
        data-nexa-overlay-root="true"
        className="contents"
      />
    </OverlayRootContext.Provider>
  );
}

export function useOverlayRoot(): HTMLElement | undefined {
  return useContext(OverlayRootContext) ?? undefined;
}
