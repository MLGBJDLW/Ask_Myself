import { useEffect, useState } from 'react';
import { Square, Waves } from 'lucide-react';

import { useMicrophoneAnalyser } from '../../features/voice/useMicrophoneAnalyser';
import { useTranslation } from '../../i18n';
import { MicrophoneWaveform } from '../voice/MicrophoneWaveform';
import { Button } from '../ui/Button';

interface MicrophoneTestPanelProps {
  deviceId: string | null;
}

/**
 * Lets the user confirm the selected microphone actually picks up their voice
 * before they rely on dictation. The stream is only opened while the test is
 * running so the OS recording indicator matches what the user started.
 */
export function MicrophoneTestPanel({ deviceId }: MicrophoneTestPanelProps) {
  const { t } = useTranslation();
  const [testing, setTesting] = useState(false);
  const { analyser, error } = useMicrophoneAnalyser(deviceId, testing);

  // Switching devices mid-test would keep showing the old bars until the new
  // stream opens, so stop and let the user restart against the new device.
  useEffect(() => setTesting(false), [deviceId]);

  useEffect(() => {
    if (error) setTesting(false);
  }, [error]);

  return (
    <div
      data-testid="microphone-test-panel"
      data-testing={testing ? 'true' : 'false'}
      className="mt-3 space-y-2 rounded-lg border border-border/70 bg-surface-1/50 p-3"
    >
      <div className="flex flex-wrap items-center gap-3">
        <Button
          variant="secondary"
          size="sm"
          icon={testing ? <Square size={14} /> : <Waves size={14} />}
          onClick={() => setTesting((value) => !value)}
        >
          {testing ? t('voice.microphoneTestStop') : t('voice.microphoneTestStart')}
        </Button>
        <MicrophoneWaveform
          analyser={testing ? analyser : null}
          barCount={28}
          className={`flex-1 ${testing && analyser ? 'text-accent' : 'text-text-tertiary/50'}`}
          label={t('voice.waveformLabel')}
        />
      </div>
      <p className="text-xs leading-5 text-text-tertiary">
        {error === 'permission_denied'
          ? t('voice.permissionDenied')
          : error === 'unavailable'
            ? t('voice.microphoneTestUnavailable')
            : t('voice.microphoneTestDesc')}
      </p>
    </div>
  );
}
