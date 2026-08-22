import { describe, expect, it, vi } from 'vitest';
import { Mp4Engine } from '../../src/core/engines/Mp4Engine';

function video(canPlay: CanPlayTypeResult = 'probably') {
  const element = document.createElement('video');
  element.canPlayType = vi.fn(() => canPlay);
  element.pause = vi.fn();
  element.load = vi.fn();
  return element;
}

describe('Mp4Engine', () => {
  it('attaches a finalized MP4 URL to the native video element', () => {
    const element = video();
    const engine = new Mp4Engine();

    engine.attach(element, {
      protocol: 'mp4',
      url: 'http://127.0.0.1/record.mp4',
      mimeCodec: 'video/mp4',
      rateMode: 'local-file',
    });

    expect(element.src).toContain('/record.mp4');
    engine.destroy();
    expect(element.getAttribute('src')).toBeNull();
  });

  it('rejects MP4 codecs unsupported by the browser', () => {
    const engine = new Mp4Engine();
    expect(() => engine.attach(video(''), {
      protocol: 'mp4',
      url: 'http://127.0.0.1/record.mp4',
      mimeCodec: 'video/mp4; codecs="hvc1"',
    })).toThrow('UNSUPPORTED_CODEC');
  });
});
