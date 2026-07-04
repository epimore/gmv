import { GmvErrorCode } from '../utils/ErrorCode';
import type { GmvSource } from '../types';
import { BaseEngine } from './BaseEngine';

export class FlvEngine extends BaseEngine {
  readonly protocol = 'flv' as const;
  private player?: any;

  async attach(video: HTMLVideoElement, source: GmvSource): Promise<void> {
    this.video = video;

    let mpegts: any;
    try {
      mpegts = (await import(/* @vite-ignore */ 'mpegts.js')).default;
    } catch {
      throw new Error(`${GmvErrorCode.EngineLoadFailed}: mpegts.js 未安装或加载失败`);
    }

    if (!mpegts.getFeatureList?.().mseLivePlayback) {
      throw new Error(`${GmvErrorCode.UnsupportedProtocol}: 当前浏览器不支持 MSE FLV 播放`);
    }

    this.player = mpegts.createPlayer(
      {
        type: 'flv',
        isLive: true,
        url: source.url,
        hasAudio: source.hasAudio !== false,
        hasVideo: true,
      },
      {
        enableStashBuffer: false,
        liveBufferLatencyChasing: true,
        autoCleanupSourceBuffer: true,
      },
    );

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
