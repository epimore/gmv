import { BrowserProbe } from '../capability/BrowserProbe';
import { GmvErrorCode } from '../utils/ErrorCode';
import type { GmvSource } from '../types';
import { BaseEngine } from './BaseEngine';

export class HlsEngine extends BaseEngine {
  readonly protocol = 'hls' as const;
  private hls?: any;

  async attach(video: HTMLVideoElement, source: GmvSource): Promise<void> {
    this.video = video;

    let Hls: any;
    try {
      Hls = (await import('hls.js')).default;
    } catch {
      if (BrowserProbe.canNativeHls(video)) {
        video.src = source.url;
        return;
      }
      throw new Error(`${GmvErrorCode.EngineLoadFailed}: hls.js 未安装或加载失败`);
    }

    if (Hls.isSupported()) {
      this.hls = new Hls({
        lowLatencyMode: true,
        liveSyncDurationCount: 3,
        backBufferLength: 30,
      });
      this.hls.loadSource(source.url);
      this.hls.attachMedia(video);
      return;
    }

    if (BrowserProbe.canNativeHls(video)) {
      video.src = source.url;
      return;
    }

    throw new Error(`${GmvErrorCode.UnsupportedProtocol}: 当前浏览器不支持 HLS`);
  }

  destroy(): void {
    this.hls?.destroy();
    this.hls = undefined;
    if (this.video) this.video.removeAttribute('src');
    this.video = undefined;
  }
}
