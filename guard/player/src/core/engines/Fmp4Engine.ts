import { BrowserProbe } from '../capability/BrowserProbe';
import { GmvErrorCode } from '../utils/ErrorCode';
import type { GmvSource } from '../types';
import { BaseEngine } from './BaseEngine';

export class Fmp4Engine extends BaseEngine {
  readonly protocol = 'fmp4' as const;
  private mediaSource?: MediaSource;
  private sourceBuffer?: SourceBuffer;
  private aborter?: AbortController;
  private objectUrl?: string;
  private readonly queue: ArrayBuffer[] = [];
  private destroyed = false;

  async attach(video: HTMLVideoElement, source: GmvSource): Promise<void> {
    this.video = video;

    if (!BrowserProbe.canUseMse()) {
      throw new Error(`${GmvErrorCode.MediaSourceUnavailable}: 当前浏览器不支持 MediaSource`);
    }
    if (!source.mimeCodec) {
      throw new Error(`${GmvErrorCode.UnsupportedCodec}: FMP4 播放必须提供 mimeCodec`);
    }
    if (!MediaSource.isTypeSupported(source.mimeCodec)) {
      throw new Error(`${GmvErrorCode.UnsupportedCodec}: 当前浏览器不支持 ${source.mimeCodec}`);
    }

    this.destroyed = false;
    this.mediaSource = new MediaSource();
    this.objectUrl = URL.createObjectURL(this.mediaSource);
    video.src = this.objectUrl;

    await new Promise<void>((resolve, reject) => {
      const mediaSource = this.mediaSource;
      if (!mediaSource) return reject(new Error(GmvErrorCode.MediaSourceUnavailable));

      const onOpen = () => {
        mediaSource.removeEventListener('sourceopen', onOpen);
        try {
          this.sourceBuffer = mediaSource.addSourceBuffer(source.mimeCodec!);
          this.sourceBuffer.mode = 'segments';
          this.sourceBuffer.addEventListener('updateend', this.flush);
          resolve();
        } catch (error) {
          reject(error);
        }
      };

      mediaSource.addEventListener('sourceopen', onOpen);
    });

    this.startFetch(source.url).catch((error) => {
      if (!this.destroyed) throw error;
    });
  }

  destroy(): void {
    this.destroyed = true;
    this.aborter?.abort();
    this.aborter = undefined;
    this.queue.length = 0;

    if (this.sourceBuffer) {
      this.sourceBuffer.removeEventListener('updateend', this.flush);
    }

    if (this.mediaSource?.readyState === 'open') {
      try {
        this.mediaSource.endOfStream();
      } catch {
        // 浏览器在关闭中的 MediaSource 可能抛错，销毁阶段可忽略。
      }
    }

    if (this.video) this.video.removeAttribute('src');
    if (this.objectUrl) URL.revokeObjectURL(this.objectUrl);

    this.sourceBuffer = undefined;
    this.mediaSource = undefined;
    this.objectUrl = undefined;
    this.video = undefined;
  }

  private async startFetch(url: string): Promise<void> {
    this.aborter = new AbortController();
    const response = await fetch(url, {
      signal: this.aborter.signal,
      cache: 'no-store',
    });

    if (!response.ok) {
      throw new Error(`${GmvErrorCode.StreamOpenFailed}: HTTP ${response.status}`);
    }
    if (!response.body) {
      throw new Error(`${GmvErrorCode.StreamReadFailed}: 响应不支持 ReadableStream`);
    }

    const reader = response.body.getReader();
    while (!this.destroyed) {
      const { done, value } = await reader.read();
      if (done) break;
      if (!value || value.byteLength === 0) continue;

      this.queue.push(value.buffer.slice(value.byteOffset, value.byteOffset + value.byteLength));
      this.flush();
    }
  }

  private readonly flush = (): void => {
    if (this.destroyed || !this.sourceBuffer || this.sourceBuffer.updating) return;

    const chunk = this.queue.shift();
    if (!chunk) return;

    try {
      this.sourceBuffer.appendBuffer(chunk);
    } catch (error) {
      this.queue.unshift(chunk);
      throw error;
    }
  };
}
