import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { Fmp4Engine } from '../../src/core/engines/Fmp4Engine';

class FakeSourceBuffer extends EventTarget {
  mode: AppendMode = 'segments';
  updating = false;
  appendBuffer = vi.fn();
}

class FakeMediaSource extends EventTarget {
  static readonly instances: FakeMediaSource[] = [];
  static isTypeSupported() { return true; }

  readyState = 'closed';
  readonly sourceBuffer = new FakeSourceBuffer();

  constructor() {
    super();
    FakeMediaSource.instances.push(this);
    queueMicrotask(() => {
      this.readyState = 'open';
      this.dispatchEvent(new Event('sourceopen'));
    });
  }

  addSourceBuffer() {
    return this.sourceBuffer;
  }

  endOfStream() {
    this.readyState = 'ended';
  }
}

function video() {
  const element = document.createElement('video');
  element.pause = vi.fn();
  element.load = vi.fn();
  return element;
}

beforeEach(() => {
  FakeMediaSource.instances.length = 0;
  vi.stubGlobal('MediaSource', FakeMediaSource);
  vi.stubGlobal('fetch', vi.fn(() => new Promise<Response>(() => {})));
  vi.stubGlobal('URL', {
    ...URL,
    createObjectURL: vi.fn(() => 'blob:gmv-fmp4'),
    revokeObjectURL: vi.fn(),
  });
});

afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

describe('Fmp4Engine errors', () => {
  it('distinguishes missing codec metadata from browser codec support', async () => {
    const engine = new Fmp4Engine();

    await expect(engine.attach(video(), {
      protocol: 'fmp4',
      url: 'http://127.0.0.1/live.fmp4',
    })).rejects.toThrow('CODEC_METADATA_MISSING');
  });

  it('reports a browser codec capability failure after metadata is present', async () => {
    vi.spyOn(FakeMediaSource, 'isTypeSupported').mockReturnValue(false);
    const engine = new Fmp4Engine();

    await expect(engine.attach(video(), {
      protocol: 'fmp4',
      url: 'http://127.0.0.1/live.fmp4',
      mimeCodec: 'video/mp4; codecs="hvc1.1.6.L123.B0, mp4a.40.2"',
    })).rejects.toThrow('UNSUPPORTED_CODEC');
  });

  it('forwards fetch failures to the video lifecycle', async () => {
    vi.stubGlobal('fetch', vi.fn().mockRejectedValue(new Error('fMP4 fetch failed')));
    const element = video();
    const onError = vi.fn();
    element.addEventListener('error', onError);
    const engine = new Fmp4Engine();

    await engine.attach(element, {
      protocol: 'fmp4',
      url: 'http://127.0.0.1/live.fmp4',
      mimeCodec: 'video/mp4; codecs="avc1.42E01E"',
    });
    await vi.waitFor(() => expect(onError).toHaveBeenCalledOnce());

    expect((onError.mock.calls[0][0] as ErrorEvent).message).toBe('fMP4 fetch failed');
    engine.destroy();
  });

  it('forwards SourceBuffer failures to the video lifecycle', async () => {
    const element = video();
    const onError = vi.fn();
    element.addEventListener('error', onError);
    const engine = new Fmp4Engine();

    await engine.attach(element, {
      protocol: 'fmp4',
      url: 'http://127.0.0.1/live.fmp4',
      mimeCodec: 'video/mp4; codecs="avc1.42E01E"',
    });
    FakeMediaSource.instances[0].sourceBuffer.dispatchEvent(new Event('error'));

    expect(onError).toHaveBeenCalledOnce();
    expect((onError.mock.calls[0][0] as ErrorEvent).message).toBe('fMP4 SourceBuffer error');
    engine.destroy();
  });
});
