import type { GmvSource } from '../types';
import mpegts from 'mpegts.js';

export class BrowserProbe {
  static canUseMse(): boolean {
    return typeof window !== 'undefined' && typeof window.MediaSource !== 'undefined';
  }

  static canUseFetchStream(): boolean {
    return typeof window !== 'undefined' && typeof window.fetch === 'function' && typeof ReadableStream !== 'undefined';
  }

  static canNativeHls(video: HTMLVideoElement): boolean {
    return video.canPlayType('application/vnd.apple.mpegurl') !== '';
  }

  static shouldUseNativeHls(video: HTMLVideoElement): boolean {
    if (!this.canNativeHls(video)) return false;
    if (!this.canUseMse()) return true;
    return typeof navigator !== 'undefined' && navigator.vendor === 'Apple Computer, Inc.';
  }

  static canPlayFmp4(source: GmvSource): boolean {
    if (!this.canUseMse()) return false;
    if (!source.mimeCodec) return true;
    return MediaSource.isTypeSupported(source.mimeCodec);
  }

  static canPlayFlv(): boolean {
    return !!mpegts.getFeatureList?.().mseLivePlayback;
  }

  static canPlayMp4(video: HTMLVideoElement, source: GmvSource): boolean {
    return video.canPlayType(source.mimeCodec ?? 'video/mp4') !== '';
  }

  static canTrySource(video: HTMLVideoElement, source: GmvSource): boolean {
    if (source.protocol === 'fmp4') return this.canUseFetchStream() && this.canPlayFmp4(source);
    if (source.protocol === 'hls') return this.canUseMse() || this.canNativeHls(video);
    if (source.protocol === 'flv') return this.canPlayFlv();
    if (source.protocol === 'mp4') return this.canPlayMp4(video, source);
    return false;
  }
}
