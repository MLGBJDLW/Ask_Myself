import { useEffect, useRef, useState } from 'react';
import { useDisplayPreferences } from './displayPreferences';
import { TextPresentation } from './streaming/textPresentation';

export function useStreamingPresentation(content: string, streaming: boolean, reduceMotion = false) {
  const { streamingMode } = useDisplayPreferences();
  const [presented, setPresented] = useState(streaming ? '' : content);
  const shown = useRef(presented);
  const latest = useRef({ content, streaming });
  latest.current = { content, streaming };
  const controller = useRef<TextPresentation | null>(null);
  useEffect(() => {
    const projection = new TextPresentation(shown.current, streamingMode, reduceMotion, text => {
      shown.current = text;
      setPresented(text);
    });
    controller.current = projection;
    projection.update(latest.current.content, latest.current.streaming);
    return () => { projection.dispose(); if (controller.current === projection) controller.current = null; };
  }, [streamingMode, reduceMotion]);
  useEffect(() => { controller.current?.update(content, streaming); }, [content, streaming]);
  return !streaming || !content.startsWith(presented) ? content : presented;
}
