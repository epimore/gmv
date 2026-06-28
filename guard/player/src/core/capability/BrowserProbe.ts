import type { GmvSource } from '../types';

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

  static canPlayFmp4(source: GmvSource): boolean {
    if (!this.canUseMse()) return false;
    if (!source.mimeCodec) return true;
    return MediaSource.isTypeSupported(source.mimeCodec);
  }

  static canTrySource(video: HTMLVideoElement, source: GmvSource): boolean {
    if (source.protocol === 'fmp4') return this.canUseFetchStream() && this.canPlayFmp4(source);
    if (source.protocol === 'hls') return this.canUseMse() || this.canNativeHls(video);
    return this.canUseMse();
  }
}
