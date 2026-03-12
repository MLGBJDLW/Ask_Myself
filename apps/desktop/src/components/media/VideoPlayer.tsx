import { useRef, useEffect, useCallback, useState, useImperativeHandle, forwardRef } from 'react';
import { convertFileSrc } from '@tauri-apps/api/core';
import { useTranslation } from '../../i18n';

interface VideoPlayerProps {
  src: string;
  thumbnailSrc?: string;
  initialTime?: number;
  onTimeUpdate?: (currentTimeMs: number) => void;
  className?: string;
}

export interface VideoPlayerHandle {
  seekTo: (seconds: number) => void;
}

export const VideoPlayer = forwardRef<VideoPlayerHandle, VideoPlayerProps>(
  function VideoPlayer(
    { src, thumbnailSrc, initialTime, onTimeUpdate, className = '' },
    ref,
  ) {
    const { t } = useTranslation();
    const videoRef = useRef<HTMLVideoElement>(null);
    const [isPlaying, setIsPlaying] = useState(false);

    const videoSrc = convertFileSrc(src);
    const posterSrc = thumbnailSrc ? convertFileSrc(thumbnailSrc) : undefined;

    useImperativeHandle(ref, () => ({
      seekTo(seconds: number) {
        if (videoRef.current) {
          videoRef.current.currentTime = seconds;
        }
      },
    }));

    useEffect(() => {
      if (videoRef.current && initialTime != null) {
        videoRef.current.currentTime = initialTime;
      }
    }, [initialTime]);

    const handleTimeUpdate = useCallback(() => {
      if (videoRef.current && onTimeUpdate) {
        onTimeUpdate(Math.floor(videoRef.current.currentTime * 1000));
      }
    }, [onTimeUpdate]);

    return (
      <div className={`relative rounded-lg overflow-hidden bg-black ${className}`}>
        <video
          ref={videoRef}
          src={videoSrc}
          controls
          poster={posterSrc}
          className="w-full"
          onTimeUpdate={handleTimeUpdate}
          onPlay={() => setIsPlaying(true)}
          onPause={() => setIsPlaying(false)}
          aria-label={isPlaying ? t('media.pause') : t('media.play')}
        />
      </div>
    );
  },
);
