import { describe, expect, it, vi } from 'vitest';

const hlsMock = vi.hoisted(() => ({
  instances: [] as Array<{
    handlers: Map<string, (...args: any[]) => void>;
    loadSource: ReturnType<typeof vi.fn>;
    attachMedia: ReturnType<typeof vi.fn>;
    destroy: ReturnType<typeof vi.fn>;
  }>,
}));

vi.mock('hls.js', () => {
  class MockHls {
    static readonly Events = { ERROR: 'error' };
    static isSupported() { return true; }

    readonly handlers = new Map<string, (...args: any[]) => void>();
    readonly loadSource = vi.fn();
    readonly attachMedia = vi.fn();
    readonly destroy = vi.fn();

    constructor() {
      hlsMock.instances.push(this);
    }

    on(event: string, handler: (...args: any[]) => void) {
      this.handlers.set(event, handler);
    }
  }

  return { default: MockHls };
});

import { HlsEngine } from '../../src/core/engines/HlsEngine';

describe('HlsEngine errors', () => {
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
});
