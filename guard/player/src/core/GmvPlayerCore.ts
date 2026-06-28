import { BrowserProbe } from './capability/BrowserProbe';
import { FlvEngine } from './engines/FlvEngine';
import { Fmp4Engine } from './engines/Fmp4Engine';
import { HlsEngine } from './engines/HlsEngine';
import type { GmvEngine, GmvEngineFactory, GmvPlayerCoreOptions, GmvPlayerEvents, GmvPlayerEvent, GmvProtocol, GmvSource } from './types';
import { EventBus, type EventHandler } from './utils/EventBus';
import { GmvErrorCode } from './utils/ErrorCode';

export class GmvPlayerCore {
  private readonly bus = new EventBus<GmvPlayerEvents>();
  private readonly video: HTMLVideoElement;
  private readonly engines: Record<GmvProtocol, GmvEngineFactory>;
  private engine?: GmvEngine;
  private sources: GmvSource[];
  private activeSource?: GmvSource;
  private reconnectRetry = 0;
  private destroyed = false;

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
    this.sources = sources;
    this.destroyCurrentEngine();
    this.destroyed = false;
    this.bus.emit('loading', undefined);

    const candidates = this.pickCandidates(sources);
    if (candidates.length === 0) {
      this.emitError(GmvErrorCode.NoSource, '没有可播放 source');
      return;
    }

    for (const source of candidates) {
      try {
        await this.attachSource(source);
        this.activeSource = source;
        this.reconnectRetry = 0;
        this.bus.emit('sourceChanged', { source });
        if (this.options.autoplay) await this.play();
        return;
      } catch (error) {
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
    if (!this.activeSource || this.destroyed) return;

    const maxRetries = this.options.reconnect?.maxRetries ?? 3;
    if (this.reconnectRetry >= maxRetries) {
      this.emitError(GmvErrorCode.StreamOpenFailed, '重连次数已达上限', this.activeSource);
      return;
    }

    this.reconnectRetry += 1;
    this.bus.emit('reconnecting', { retry: this.reconnectRetry, reason });
    await this.delay((this.options.reconnect?.baseDelayMs ?? 800) * this.reconnectRetry);
    await this.load([this.activeSource]);
    this.bus.emit('reconnected', undefined);
  }

  destroy(): void {
    this.destroyed = true;
    this.destroyCurrentEngine();
    this.video.removeAttribute('src');
    this.bus.emit('destroyed', undefined);
    this.bus.clear();
  }

  private async attachSource(source: GmvSource): Promise<void> {
    const factory = this.engines[source.protocol];
    if (!factory) {
      throw new Error(`${GmvErrorCode.UnsupportedProtocol}: ${source.protocol}`);
    }

    this.engine = factory();
    await this.engine.attach(this.video, source);
  }

  private pickCandidates(sources: GmvSource[]): GmvSource[] {
    return [...sources]
      .filter((source) => BrowserProbe.canTrySource(this.video, source))
      .sort((left, right) => (left.priority ?? 100) - (right.priority ?? 100));
  }

  private bindVideoEvents(): void {
    this.video.addEventListener('playing', () => {
      this.bus.emit('playing', undefined);
      this.emitStats();
    });
    this.video.addEventListener('pause', () => this.bus.emit('paused', undefined));
    this.video.addEventListener('stalled', () => {
      this.bus.emit('stalled', undefined);
      void this.reconnect('stalled');
    });
    this.video.addEventListener('error', () => {
      this.emitError(GmvErrorCode.StreamReadFailed, this.video.error?.message ?? 'video error', this.activeSource);
      void this.reconnect('video-error');
    });
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

  private destroyCurrentEngine(): void {
    this.engine?.destroy();
    this.engine = undefined;
  }

  private delay(ms: number): Promise<void> {
    return new Promise((resolve) => window.setTimeout(resolve, ms));
  }
}
