import { GmvErrorCode } from '../utils/ErrorCode';
import type { GmvSource } from '../types';
import { BaseEngine } from './BaseEngine';

export class Mp4Engine extends BaseEngine {
  readonly protocol = 'mp4' as const;

  attach(video: HTMLVideoElement, source: GmvSource): void {
    this.video = video;
    const mimeCodec = source.mimeCodec ?? 'video/mp4';
    if (video.canPlayType(mimeCodec) === '') {
      throw new Error(`${GmvErrorCode.UnsupportedCodec}: ${mimeCodec}`);
    }
    video.src = source.url;
  }

  destroy(): void {
    if (!this.video) return;
    this.video.pause();
    this.video.removeAttribute('src');
    this.video.load();
    this.video = undefined;
  }
}
