import { BrowserProbe } from './capability/BrowserProbe';
import { FlvEngine } from './engines/FlvEngine';
import { Fmp4Engine } from './engines/Fmp4Engine';
import { HlsEngine } from './engines/HlsEngine';
import type { GmvEngine, GmvEngineFactory, GmvPlayerCoreOptions, GmvPlayerEvents, GmvPlayerEvent, GmvProtocol, GmvSource } from './types';
import { EventBus, type EventHandler } from './utils/EventBus';
import { GmvErrorCode } from './utils/ErrorCode';

const STALL_GRACE_MS = 5_000;
const STALL_CHECK_INTERVAL_MS = 1_000;
const STABLE_PLAYBACK_MS = 10_000;
const MAX_RECONNECT_DELAY_MS = 10_000;

export class GmvPlayerCore {
  private readonly bus = new EventBus<GmvPlayerEvents>();
  private readonly video: HTMLVideoElement;
  private readonly engines: Record<GmvProtocol, GmvEngineFactory>;
  private engine?: GmvEngine;
  private sources: GmvSource[];
  private activeSource?: GmvSource;
  private reconnectRetry = 0;
  private reconnectInFlight = false;
  private destroyed = false;
  private loadVersion = 0;
  private stallWatch?: number;
  private stablePlaybackTimer?: number;
  private stallCurrentTime = 0;
  private stallBufferEnd = 0;
  private readonly videoCleanups: Array<() => void> = [];

  constructor(private readonly options: GmvPlayerCoreOptions) {
    this.video = options.video;
    this.sources = options.sources;
    this.video.muted = options.muted ?? this.video.muted;
    this.engines = {
      flv: () => new FlvEngine(),
      fmp4: () => new Fmp4Engine(),
      hls: () => new HlsEngine(),
    };
    this.bindVideoEvents();
  }

  on<K extends GmvPlayerEvent>(event: K, handler: EventHandler<GmvPlayerEvents[K]>): () => void {
    return this.bus.on(event, handler);
  }

  async load(sources = this.sources): Promise<void> {
    const version = ++this.loadVersion;
    this.sources = sources;
    this.clearStallWatch();
    this.clearStablePlaybackTimer();
    this.destroyCurrentEngine();
    this.activeSource = undefined;
    this.destroyed = false;
    this.bus.emit('loading', undefined);

    const candidates = this.pickCandidates(sources);
    if (candidates.length === 0) {
      this.emitError(GmvErrorCode.NoSource, '没有可播放 source');
      return;
    }

    for (const source of candidates) {
      if (this.destroyed || version !== this.loadVersion) return;
      try {
        const engine = await this.attachSource(source);
        if (this.destroyed || version !== this.loadVersion) {
          engine.destroy();
          return;
        }
        this.engine = engine;
        this.activeSource = source;
        this.bus.emit('sourceChanged', { source });
        if (this.options.autoplay) {
          try {
            await this.play();
          } catch (error) {
            if (this.destroyed || version !== this.loadVersion) return;
            this.emitError(GmvErrorCode.StreamOpenFailed, error instanceof Error ? error.message : '自动播放失败', source);
          }
        }
        return;
      } catch (error) {
        if (this.destroyed || version !== this.loadVersion) return;
        this.destroyCurrentEngine();
        this.emitError(GmvErrorCode.StreamOpenFailed, error instanceof Error ? error.message : '播放源打开失败', source);
        if (this.options.fallback === false) return;
      }
    }
  }

  play(): Promise<void> | void {
    return this.engine?.play() ?? this.video.play();
  }

  pause(): void {
    this.engine?.pause();
  }

  async switchSource(source: GmvSource): Promise<void> {
    await this.load([source, ...this.sources.filter((item) => item.url !== source.url)]);
  }

  async reconnect(reason = 'manual'): Promise<void> {
    if (!this.activeSource || this.destroyed || this.reconnectInFlight) return;

    const maxRetries = this.options.reconnect?.maxRetries ?? 3;
    if (this.reconnectRetry >= maxRetries) {
      this.emitError(GmvErrorCode.StreamOpenFailed, '重连次数已达上限', this.activeSource);
      return;
    }

    this.reconnectRetry += 1;
    const source = this.activeSource;
    this.clearStallWatch();
    this.clearStablePlaybackTimer();
    this.reconnectInFlight = true;
    this.bus.emit('reconnecting', { retry: this.reconnectRetry, reason });
    const version = this.loadVersion;
    const baseDelayMs = this.options.reconnect?.baseDelayMs ?? 800;
    const delayMs = Math.min(baseDelayMs * 2 ** (this.reconnectRetry - 1), MAX_RECONNECT_DELAY_MS);
    try {
      await this.delay(delayMs);
      if (this.destroyed || version !== this.loadVersion) return;
      await this.load([source]);
      if (this.destroyed || !this.engine) return;
      this.bus.emit('reconnected', undefined);
    } finally {
      this.reconnectInFlight = false;
    }
  }

  destroy(): void {
    this.destroyed = true;
    this.loadVersion += 1;
    this.reconnectInFlight = false;
    this.clearStallWatch();
    this.clearStablePlaybackTimer();
    this.destroyCurrentEngine();
    while (this.videoCleanups.length) this.videoCleanups.pop()?.();
    this.video.pause();
    this.video.removeAttribute('src');
    this.video.load();
    this.bus.emit('destroyed', undefined);
    this.bus.clear();
  }

  private async attachSource(source: GmvSource): Promise<GmvEngine> {
    const factory = this.engines[source.protocol];
    if (!factory) {
      throw new Error(`${GmvErrorCode.UnsupportedProtocol}: ${source.protocol}`);
    }

    const engine = factory();
    await engine.attach(this.video, source);
    return engine;
  }

  private pickCandidates(sources: GmvSource[]): GmvSource[] {
    return [...sources]
      .filter((source) => BrowserProbe.canTrySource(this.video, source))
      .sort((left, right) => (left.priority ?? 100) - (right.priority ?? 100));
  }

  private bindVideoEvents(): void {
    const onPlaying = () => {
      this.clearStallWatch();
      this.bus.emit('playing', undefined);
      this.emitStats();
      this.scheduleStablePlaybackReset();
    };
    const onPause = () => {
      this.clearStallWatch();
      this.clearStablePlaybackTimer();
      this.bus.emit('paused', undefined);
    };
    const onStalled = () => {
      if (this.destroyed) return;
      this.bus.emit('stalled', undefined);
      this.startStallWatch();
    };
    const onError = () => {
      if (this.destroyed) return;
      this.clearStallWatch();
      this.clearStablePlaybackTimer();
      this.emitError(GmvErrorCode.StreamReadFailed, this.video.error?.message ?? 'video error', this.activeSource);
      void this.reconnect('video-error');
    };
    this.video.addEventListener('playing', onPlaying);
    this.video.addEventListener('pause', onPause);
    this.video.addEventListener('stalled', onStalled);
    this.video.addEventListener('error', onError);
    this.videoCleanups.push(
      () => this.video.removeEventListener('playing', onPlaying),
      () => this.video.removeEventListener('pause', onPause),
      () => this.video.removeEventListener('stalled', onStalled),
      () => this.video.removeEventListener('error', onError),
    );
  }

  private emitStats(): void {
    if (!this.activeSource) return;
    const buffered = this.video.buffered;
    const bufferSeconds = buffered.length > 0 ? Math.max(0, buffered.end(buffered.length - 1) - this.video.currentTime) : 0;
    this.bus.emit('stats', {
      protocol: this.activeSource.protocol,
      codec: this.activeSource.codec,
      bufferSeconds,
    });
  }

  private emitError(code: string, message: string, source?: GmvSource): void {
    this.bus.emit('error', { code, message, source });
  }

  private startStallWatch(): void {
    if (this.stallWatch || !this.activeSource) return;

    const startedAt = Date.now();
    this.stallCurrentTime = this.video.currentTime;
    this.stallBufferEnd = this.bufferedEnd();
    this.clearStablePlaybackTimer();
    this.stallWatch = window.setInterval(() => {
      if (this.destroyed || !this.activeSource) {
        this.clearStallWatch();
        return;
      }

      const currentTime = this.video.currentTime;
      const bufferEnd = this.bufferedEnd();
      if (currentTime > this.stallCurrentTime + 0.05 || bufferEnd > this.stallBufferEnd + 0.05) {
        this.clearStallWatch();
        return;
      }

      if (Date.now() - startedAt >= STALL_GRACE_MS) {
        this.clearStallWatch();
        void this.reconnect('stalled-timeout');
      }
    }, STALL_CHECK_INTERVAL_MS);
  }

  private scheduleStablePlaybackReset(): void {
    if (this.stablePlaybackTimer || this.reconnectRetry === 0) return;

    this.stablePlaybackTimer = window.setTimeout(() => {
      this.stablePlaybackTimer = undefined;
      if (!this.destroyed && !this.video.paused) this.reconnectRetry = 0;
    }, STABLE_PLAYBACK_MS);
  }

  private clearStallWatch(): void {
    if (this.stallWatch === undefined) return;
    window.clearInterval(this.stallWatch);
    this.stallWatch = undefined;
  }

  private clearStablePlaybackTimer(): void {
    if (this.stablePlaybackTimer === undefined) return;
    window.clearTimeout(this.stablePlaybackTimer);
    this.stablePlaybackTimer = undefined;
  }

  private bufferedEnd(): number {
    const buffered = this.video.buffered;
    return buffered.length > 0 ? buffered.end(buffered.length - 1) : 0;
  }

  private destroyCurrentEngine(): void {
    this.engine?.destroy();
    this.engine = undefined;
  }

  private delay(ms: number): Promise<void> {
    return new Promise((resolve) => window.setTimeout(resolve, ms));
  }
}
