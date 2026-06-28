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
    this.player?.destroy();
    this.player = undefined;
    this.video = undefined;
  }
}
