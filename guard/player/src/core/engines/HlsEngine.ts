import { BrowserProbe } from '../capability/BrowserProbe';
import { GmvErrorCode } from '../utils/ErrorCode';
import type { GmvSource } from '../types';
import { BaseEngine } from './BaseEngine';

const MEDIA_RECOVERY_GRACE_MS = 5_000;

interface HlsErrorData {
  fatal?: boolean;
  type?: string;
  details?: string;
  reason?: string;
  error?: { message?: string };
  frag?: { url?: string };
  part?: { url?: string };
  response?: { code?: number };
}

export class HlsEngine extends BaseEngine {
  readonly protocol = 'hls' as const;
  private hls?: any;
  private mediaRecoveryAt?: number;
  private destroyed = false;
  private attaching = false;

  async attach(video: HTMLVideoElement, source: GmvSource): Promise<void> {
    this.destroyed = false;
    this.attaching = true;
    this.video = video;

    if (BrowserProbe.shouldUseNativeHls(video)) {
      this.attaching = false;
      video.src = source.url;
      return;
    }

    let Hls: any;
    try {
      Hls = (await import('hls.js')).default;
    } catch {
      if (this.destroyed) return;
      throw new Error(`${GmvErrorCode.EngineLoadFailed}: hls.js 未安装或加载失败`);
    }
    if (this.destroyed || this.video !== video) return;

    if (Hls.isSupported()) {
      this.hls = new Hls({
        lowLatencyMode: isLowLatencyPlaylist(source.url),
        backBufferLength: 30,
      });
      this.attaching = false;
      this.hls.on(Hls.Events.ERROR, (_event: unknown, data: HlsErrorData) => {
        if (!data?.fatal || !this.video) return;
        if (data.type === Hls.ErrorTypes.MEDIA_ERROR && this.recoverMediaError()) return;
        console.error('[gmv-player][hls]', hlsErrorDiagnostic(data));
        const message = hlsErrorMessage(data);
        this.video.dispatchEvent(new ErrorEvent('error', { message }));
      });
      this.hls.loadSource(source.url);
      this.hls.attachMedia(video);
      return;
    }

    throw new Error(`${GmvErrorCode.UnsupportedProtocol}: 当前浏览器不支持 HLS`);
  }

  recoverMediaError(): boolean {
    if (this.attaching && !this.destroyed) return true;
    if (this.destroyed || !this.hls) return false;
    const now = Date.now();
    if (this.mediaRecoveryAt !== undefined) {
      const elapsedMs = now - this.mediaRecoveryAt;
      return elapsedMs < MEDIA_RECOVERY_GRACE_MS;
    }
    this.mediaRecoveryAt = now;
    this.hls.recoverMediaError();
    return true;
  }

  destroy(): void {
    this.destroyed = true;
    this.attaching = false;
    this.hls?.destroy();
    this.hls = undefined;
    this.mediaRecoveryAt = undefined;
    if (this.video) this.video.removeAttribute('src');
    this.video = undefined;
  }
}

function hlsErrorMessage(data: HlsErrorData): string {
  const category = [data.type, data.details].filter(Boolean).join('/');
  const reason = data.reason || data.error?.message;
  const status = data.response?.code ? `HTTP ${data.response.code}` : undefined;
  const resource = mediaResourcePath(data.part?.url || data.frag?.url);
  const context = [reason, status, resource].filter(Boolean).join('; ');
  if (category && context) return `${category}: ${context}`;
  return category || context || 'HLS fatal error';
}

function hlsErrorDiagnostic(data: HlsErrorData) {
  return {
    fatal: data.fatal ?? false,
    type: data.type,
    details: data.details,
    reason: data.reason || data.error?.message,
    status: data.response?.code,
    resource: mediaResourcePath(data.part?.url || data.frag?.url),
  };
}

function mediaResourcePath(resourceUrl?: string): string | undefined {
  if (!resourceUrl) return undefined;
  try {
    return new URL(resourceUrl, window.location.href).pathname;
  } catch {
    return resourceUrl.split('?', 1)[0];
  }
}

function isLowLatencyPlaylist(sourceUrl: string): boolean {
  try {
    return new URL(sourceUrl, window.location.href).pathname.endsWith('.ll.m3u8');
  } catch {
    return sourceUrl.split('?', 1)[0].endsWith('.ll.m3u8');
  }
}
