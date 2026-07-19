import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

const hlsMock = vi.hoisted(() => ({
  videoErrorOnAttach: false,
  instances: [] as Array<{
    handlers: Map<string, (...args: any[]) => void>;
    loadSource: ReturnType<typeof vi.fn>;
    attachMedia: ReturnType<typeof vi.fn>;
    destroy: ReturnType<typeof vi.fn>;
    recoverMediaError: ReturnType<typeof vi.fn>;
    options?: Record<string, unknown>;
  }>,
}));

vi.mock('hls.js', () => {
  class MockHls {
    static readonly Events = { ERROR: 'error' };
    static readonly ErrorTypes = { MEDIA_ERROR: 'mediaError' };
    static isSupported() { return true; }

    readonly handlers = new Map<string, (...args: any[]) => void>();
    readonly loadSource = vi.fn();
    readonly attachMedia = vi.fn((video: HTMLVideoElement) => {
      if (hlsMock.videoErrorOnAttach) {
        video.dispatchEvent(new ErrorEvent('error', {
          message: 'PipelineStatus::DEMUXER_ERROR_COULD_NOT_PARSE',
        }));
      }
    });
    readonly destroy = vi.fn();
    readonly recoverMediaError = vi.fn();
    readonly options?: Record<string, unknown>;

    constructor(options?: Record<string, unknown>) {
      this.options = options;
      hlsMock.instances.push(this);
    }

    on(event: string, handler: (...args: any[]) => void) {
      this.handlers.set(event, handler);
    }
  }

  return { default: MockHls };
});

import { HlsEngine } from '../../src/core/engines/HlsEngine';
import { BrowserProbe } from '../../src/core/capability/BrowserProbe';
import { GmvPlayerCore } from '../../src/core/GmvPlayerCore';

describe('HlsEngine errors', () => {
  beforeEach(() => {
    hlsMock.videoErrorOnAttach = false;
    vi.spyOn(console, 'error').mockImplementation(() => undefined);
  });

  afterEach(() => {
    vi.useRealTimers();
    vi.restoreAllMocks();
  });

  it('parses the server LL-HLS playlist profile with hls.js', async () => {
    const { M3U8Parser } = await vi.importActual<typeof import('hls.js')>('hls.js');
    const playlist = `#EXTM3U
#EXT-X-VERSION:10
#EXT-X-TARGETDURATION:4
#EXT-X-PART-INF:PART-TARGET=0.500000
#EXT-X-SERVER-CONTROL:CAN-BLOCK-RELOAD=YES,PART-HOLD-BACK=1.500000
#EXT-X-MEDIA-SEQUENCE:3
#EXT-X-MAP:URI="live.hmp4?gmv-token=token"
#EXT-X-PART:DURATION=0.450000,URI="live-part-3-0.m4s?gmv-token=token",INDEPENDENT=YES
#EXT-X-PART:DURATION=0.480000,URI="live-part-3-1.m4s?gmv-token=token"
#EXTINF:0.930000,
live-3.m4s?gmv-token=token
#EXT-X-PART:DURATION=0.450000,URI="live-part-4-0.m4s?gmv-token=token",INDEPENDENT=YES
#EXT-X-PRELOAD-HINT:TYPE=PART,URI="live-part-4-1.m4s?gmv-token=token"
`;

    const details = M3U8Parser.parseLevelPlaylist(
      playlist,
      'http://127.0.0.1/live.m3u8',
      0,
      'main',
      0,
      null,
    );

    expect(details.canBlockReload).toBe(true);
    expect(details.partTarget).toBe(0.5);
    expect(details.partHoldBack).toBe(1.5);
    expect(details.partList).toHaveLength(3);
    expect(details.preloadHint?.URI).toContain('live-part-4-1.m4s');
  });

  it('uses server LL-HLS hold-back without overriding it by segment count', async () => {
    hlsMock.instances.length = 0;
    const engine = new HlsEngine();

    await engine.attach(document.createElement('video'), {
      protocol: 'hls',
      url: 'http://127.0.0.1/live.ll.m3u8',
      codec: 'h264',
    });

    expect(hlsMock.instances[0].options).toMatchObject({ lowLatencyMode: true });
    expect(hlsMock.instances[0].options).not.toHaveProperty('liveSyncDurationCount');
    engine.destroy();
  });

  it('disables low-latency mode for the ordinary HLS endpoint', async () => {
    hlsMock.instances.length = 0;
    const engine = new HlsEngine();

    await engine.attach(document.createElement('video'), {
      protocol: 'hls',
      url: 'http://127.0.0.1/live.m3u8',
      codec: 'h264',
    });

    expect(hlsMock.instances[0].options).toMatchObject({ lowLatencyMode: false });
    engine.destroy();
  });

  it('uses native HLS when the browser has no MSE path', async () => {
    hlsMock.instances.length = 0;
    const nativeHls = vi.spyOn(BrowserProbe, 'canNativeHls').mockReturnValue(true);
    const mse = vi.spyOn(BrowserProbe, 'canUseMse').mockReturnValue(false);
    const video = document.createElement('video');
    const engine = new HlsEngine();

    await engine.attach(video, {
      protocol: 'hls',
      url: 'http://127.0.0.1/live.m3u8',
      codec: 'h264',
    });

    expect(video.src).toBe('http://127.0.0.1/live.m3u8');
    expect(hlsMock.instances).toHaveLength(0);
    nativeHls.mockRestore();
    mse.mockRestore();
    engine.destroy();
  });

  it('uses hls.js when a Chromium browser claims native HLS support', async () => {
    hlsMock.instances.length = 0;
    const nativeHls = vi.spyOn(BrowserProbe, 'canNativeHls').mockReturnValue(true);
    const mse = vi.spyOn(BrowserProbe, 'canUseMse').mockReturnValue(true);
    const vendor = vi.spyOn(navigator, 'vendor', 'get').mockReturnValue('Google Inc.');
    const video = document.createElement('video');
    const engine = new HlsEngine();

    await engine.attach(video, {
      protocol: 'hls',
      url: 'http://127.0.0.1/live.ll.m3u8',
      codec: 'h264',
    });

    expect(hlsMock.instances).toHaveLength(1);
    expect(hlsMock.instances[0].loadSource).toHaveBeenCalledWith('http://127.0.0.1/live.ll.m3u8');
    nativeHls.mockRestore();
    mse.mockRestore();
    vendor.mockRestore();
    engine.destroy();
  });

  it('keeps native HLS for Apple browsers with MSE support', async () => {
    hlsMock.instances.length = 0;
    const nativeHls = vi.spyOn(BrowserProbe, 'canNativeHls').mockReturnValue(true);
    const mse = vi.spyOn(BrowserProbe, 'canUseMse').mockReturnValue(true);
    const vendor = vi.spyOn(navigator, 'vendor', 'get').mockReturnValue('Apple Computer, Inc.');
    const video = document.createElement('video');
    const engine = new HlsEngine();

    await engine.attach(video, {
      protocol: 'hls',
      url: 'http://127.0.0.1/live.ll.m3u8',
      codec: 'h264',
    });

    expect(video.src).toBe('http://127.0.0.1/live.ll.m3u8');
    expect(hlsMock.instances).toHaveLength(0);
    nativeHls.mockRestore();
    mse.mockRestore();
    vendor.mockRestore();
    engine.destroy();
  });

  it('does not attach a stale hls.js instance after async engine destruction', async () => {
    hlsMock.instances.length = 0;
    const video = document.createElement('video');
    const engine = new HlsEngine();

    const attaching = engine.attach(video, {
      protocol: 'hls',
      url: 'http://127.0.0.1/stale.ll.m3u8',
      codec: 'h264',
    });
    engine.destroy();
    video.src = 'blob:https://example.test/current-player';
    await attaching;

    expect(hlsMock.instances).toHaveLength(0);
    expect(video.src).toBe('blob:https://example.test/current-player');
  });

  it('forwards fatal hls.js errors to the video lifecycle', async () => {
    hlsMock.instances.length = 0;
    const video = document.createElement('video');
    const onError = vi.fn();
    video.addEventListener('error', onError);
    const engine = new HlsEngine();

    await engine.attach(video, {
      protocol: 'hls',
      url: 'http://127.0.0.1/live.m3u8',
      codec: 'h264',
    });
    hlsMock.instances[0].handlers.get('error')?.('error', {
      fatal: true,
      details: 'fragParsingError',
    });

    expect(onError).toHaveBeenCalledOnce();
    expect((onError.mock.calls[0][0] as ErrorEvent).message).toBe('fragParsingError');
    engine.destroy();
  });

  it('recovers one fatal media error before forwarding repeated failures', async () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date('2026-07-18T00:00:00Z'));
    hlsMock.instances.length = 0;
    const video = document.createElement('video');
    const onError = vi.fn();
    video.addEventListener('error', onError);
    const engine = new HlsEngine();

    await engine.attach(video, {
      protocol: 'hls',
      url: 'http://127.0.0.1/live.ll.m3u8',
      codec: 'h264',
    });
    const handler = hlsMock.instances[0].handlers.get('error')!;
    const error = {
      fatal: true,
      type: 'mediaError',
      details: 'fragParsingError',
      reason: 'invalid fragment',
    };

    handler('error', error);
    handler('error', error);
    expect(hlsMock.instances[0].recoverMediaError).toHaveBeenCalledOnce();
    expect(onError).not.toHaveBeenCalled();

    vi.advanceTimersByTime(5_001);
    handler('error', error);
    expect(onError).toHaveBeenCalledOnce();
    expect((onError.mock.calls[0][0] as ErrorEvent).message)
      .toBe('mediaError/fragParsingError: invalid fragment');
    engine.destroy();
    vi.useRealTimers();
  });

  it('recovers a video pipeline error before the player core reports failure', async () => {
    hlsMock.instances.length = 0;
    const canTrySource = vi.spyOn(BrowserProbe, 'canTrySource').mockReturnValue(true);
    const video = document.createElement('video');
    const core = new GmvPlayerCore({
      video,
      sources: [{
        protocol: 'hls',
        url: 'http://127.0.0.1/live.ll.m3u8',
        codec: 'h264',
      }],
    });
    const onError = vi.fn();
    core.on('error', onError);

    await core.load();
    video.dispatchEvent(new ErrorEvent('error', {
      message: 'PipelineStatus::DEMUXER_ERROR_COULD_NOT_PARSE',
    }));

    expect(hlsMock.instances[0].recoverMediaError).toHaveBeenCalledOnce();
    expect(onError).not.toHaveBeenCalled();
    core.destroy();
    canTrySource.mockRestore();
  });

  it('owns the HLS engine before attachMedia can emit a pipeline error', async () => {
    hlsMock.instances.length = 0;
    hlsMock.videoErrorOnAttach = true;
    const canTrySource = vi.spyOn(BrowserProbe, 'canTrySource').mockReturnValue(true);
    const video = document.createElement('video');
    const core = new GmvPlayerCore({
      video,
      sources: [{
        protocol: 'hls',
        url: 'http://127.0.0.1/live.m3u8',
        codec: 'h264',
      }],
    });
    const onError = vi.fn();
    core.on('error', onError);

    await core.load();

    expect(hlsMock.instances[0].recoverMediaError).toHaveBeenCalledOnce();
    expect(onError).not.toHaveBeenCalled();
    core.destroy();
    canTrySource.mockRestore();
  });

  it('ignores a stale video error while the current HLS attachment is pending', async () => {
    hlsMock.instances.length = 0;
    const canTrySource = vi.spyOn(BrowserProbe, 'canTrySource').mockReturnValue(true);
    const video = document.createElement('video');
    const core = new GmvPlayerCore({
      video,
      sources: [{
        protocol: 'hls',
        url: 'http://127.0.0.1/live.ll.m3u8',
        codec: 'h264',
      }],
    });
    const onError = vi.fn(() => core.destroy());
    core.on('error', onError);

    const loading = core.load();
    video.dispatchEvent(new ErrorEvent('error', {
      message: 'PipelineStatus::DEMUXER_ERROR_COULD_NOT_PARSE',
    }));
    await loading;

    expect(onError).not.toHaveBeenCalled();
    expect(hlsMock.instances[0].loadSource).toHaveBeenCalledWith('http://127.0.0.1/live.ll.m3u8');
    core.destroy();
    canTrySource.mockRestore();
  });

  it('destroys an HLS engine while its asynchronous attachment is pending', async () => {
    hlsMock.instances.length = 0;
    const canTrySource = vi.spyOn(BrowserProbe, 'canTrySource').mockReturnValue(true);
    const core = new GmvPlayerCore({
      video: document.createElement('video'),
      sources: [{
        protocol: 'hls',
        url: 'http://127.0.0.1/stale.ll.m3u8',
        codec: 'h264',
      }],
    });

    const loading = core.load();
    core.destroy();
    await loading;

    expect(hlsMock.instances).toHaveLength(0);
    canTrySource.mockRestore();
  });

  it('reports fatal resource context without exposing its query token', async () => {
    hlsMock.instances.length = 0;
    const video = document.createElement('video');
    const onError = vi.fn();
    video.addEventListener('error', onError);
    const engine = new HlsEngine();

    await engine.attach(video, {
      protocol: 'hls',
      url: 'http://127.0.0.1/live.m3u8',
      codec: 'h264',
    });
    hlsMock.instances[0].handlers.get('error')?.('error', {
      fatal: true,
      type: 'networkError',
      details: 'fragLoadError',
      response: { code: 404 },
      frag: { url: 'https://example.test/live-3.m4s?gmv-token=secret' },
    });

    const message = (onError.mock.calls[0][0] as ErrorEvent).message;
    expect(message).toContain('networkError/fragLoadError');
    expect(message).toContain('HTTP 404');
    expect(message).toContain('/live-3.m4s');
    expect(message).not.toContain('secret');
    expect(JSON.stringify(vi.mocked(console.error).mock.calls)).not.toContain('secret');
    engine.destroy();
  });
});
