import { GmvErrorCode } from '../utils/ErrorCode';
import type { GmvSource } from '../types';
import { BaseEngine } from './BaseEngine';
import mpegts from 'mpegts.js';

export class FlvEngine extends BaseEngine {
  readonly protocol = 'flv' as const;
  private player?: any;
  private readonly handleLoadingComplete = () => this.video?.dispatchEvent(new Event('ended'));

  async attach(video: HTMLVideoElement, source: GmvSource): Promise<void> {
    this.video = video;

    if (!mpegts.getFeatureList?.().mseLivePlayback) {
      throw new Error(`${GmvErrorCode.UnsupportedProtocol}: 当前浏览器不支持 MSE FLV 播放`);
    }

    const mediaDataSource: Record<string, unknown> = {
      type: 'flv',
      isLive: true,
      url: source.url,
      hasVideo: true,
    };
    // Never force an audio track to exist: mpegts.js waits for its metadata before dispatching video.
    if (source.hasAudio === false) mediaDataSource.hasAudio = false;

    this.player = mpegts.createPlayer(
      mediaDataSource,
      {
        enableStashBuffer: true,
        stashInitialSize: 512 * 1024,
        liveBufferLatencyChasing: false,
        liveSync: true,
        liveSyncTargetLatency: 1.2,
        liveSyncMaxLatency: 2,
        liveSyncPlaybackRate: 1.05,
        // mpegts.js uses a webpack-specific Blob worker that cannot reliably boot from the Vite production bundle.
        enableWorker: false,
        enableWorkerForMSE: false,
        autoCleanupSourceBuffer: true,
      },
    );

    this.player.on?.(mpegts.Events.LOADING_COMPLETE, this.handleLoadingComplete);
    this.player.attachMediaElement(video);
    this.player.load();
  }

  play(): Promise<void> | void {
    return this.player?.play();
  }

  pause(): void {
    this.player?.pause();
  }

  destroy(): void {
    try {
      this.player?.off?.(mpegts.Events.LOADING_COMPLETE, this.handleLoadingComplete);
      this.player?.pause?.();
      this.player?.unload?.();
      this.player?.detachMediaElement?.();
      this.player?.destroy?.();
    } catch {
      // mpegts.js may throw while tearing down a failed live stream; destroy should stay idempotent.
    }
    this.player = undefined;
    if (this.video) {
      this.video.pause();
      this.video.removeAttribute('src');
      this.video.load();
    }
    this.video = undefined;
  }
}
