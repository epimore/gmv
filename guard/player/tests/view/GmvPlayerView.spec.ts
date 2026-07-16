import { mount } from '@vue/test-utils';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

const players: Array<ReturnType<typeof createMpegtsPlayer>> = [];
const mpegtsMock = vi.hoisted(() => ({ createPlayer: vi.fn() }));
const hlsMock = vi.hoisted(() => ({
  instances: [] as Array<{
    loadSource: ReturnType<typeof vi.fn>;
    attachMedia: ReturnType<typeof vi.fn>;
    destroy: ReturnType<typeof vi.fn>;
  }>,
}));

vi.mock('mpegts.js', () => ({
  default: {
    getFeatureList: () => ({ mseLivePlayback: true }),
    createPlayer: mpegtsMock.createPlayer,
  },
}));

vi.mock('hls.js', () => {
  class MockHls {
    static readonly Events = { ERROR: 'error' };
    static isSupported() { return true; }

    readonly loadSource = vi.fn();
    readonly attachMedia = vi.fn();
    readonly destroy = vi.fn();

    constructor() {
      hlsMock.instances.push(this);
    }

    on() {}
  }

  return { default: MockHls };
});

import GmvPlayerView from '../../src/view/GmvPlayerView.vue';

function createMpegtsPlayer() {
  return {
    attachMediaElement: vi.fn(),
    load: vi.fn(),
    play: vi.fn(() => Promise.resolve()),
    pause: vi.fn(),
    unload: vi.fn(),
    detachMediaElement: vi.fn(),
    destroy: vi.fn(),
  };
}

function source(url: string) {
  return [{ protocol: 'flv' as const, url, codec: 'h264' as const, hasAudio: false }];
}

function hlsSource(url: string) {
  return [{ protocol: 'hls' as const, url, codec: 'h264' as const }];
}

beforeEach(() => {
  players.length = 0;
  hlsMock.instances.length = 0;
  mpegtsMock.createPlayer.mockReset();
  mpegtsMock.createPlayer.mockImplementation(() => {
    const player = createMpegtsPlayer();
    players.push(player);
    return player;
  });
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe('GmvPlayerView make-before-break', () => {
  it('远端确认倍速后同步设置本地 video playbackRate', async () => {
    const wrapper = mount(GmvPlayerView, {
      props: {
        sources: [{
          protocol: 'flv',
          url: 'http://127.0.0.1/playback.flv',
          codec: 'h264',
          rateMode: 'remote-stream',
        }],
      },
    });
    await vi.waitFor(() => expect(players).toHaveLength(1));
    const video = wrapper.find('video').element;
    video.dispatchEvent(new Event('playing'));
    await wrapper.vm.$nextTick();

    (wrapper.vm as unknown as { confirmPlaybackRate: (rate: number) => void })
      .confirmPlaybackRate(4);

    expect(video.playbackRate).toBe(4);
    wrapper.unmount();
  });

  it('新 source playing 前保留旧 engine 和旧画面', async () => {
    const wrapper = mount(GmvPlayerView, { props: { sources: source('http://127.0.0.1/old.flv') } });
    await vi.waitFor(() => expect(players).toHaveLength(1));
    const videos = wrapper.findAll('video');
    videos[0].element.dispatchEvent(new Event('playing'));
    await wrapper.vm.$nextTick();

    await wrapper.setProps({ sources: source('http://127.0.0.1/new.flv') });
    await vi.waitFor(() => expect(players).toHaveLength(2));

    expect(players[0].destroy).not.toHaveBeenCalled();
    expect(videos[0].classes()).toContain('is-active');

    videos[0].element.dispatchEvent(new Event('playing'));
    await wrapper.vm.$nextTick();

    expect(players[0].destroy).not.toHaveBeenCalled();
    expect(videos[0].classes()).toContain('is-active');

    let presentNextFrame: (() => void) | undefined;
    Object.defineProperty(videos[1].element, 'requestVideoFrameCallback', {
      configurable: true,
      value: vi.fn((callback: VideoFrameRequestCallback) => {
        presentNextFrame = () => callback(0, {} as VideoFrameCallbackMetadata);
        return 1;
      }),
    });
    Object.defineProperty(videos[1].element, 'cancelVideoFrameCallback', {
      configurable: true,
      value: vi.fn(),
    });
    videos[1].element.dispatchEvent(new Event('playing'));
    await wrapper.vm.$nextTick();

    expect(players[0].destroy).not.toHaveBeenCalled();
    expect(videos[0].classes()).toContain('is-active');

    presentNextFrame?.();
    expect(players[0].destroy).not.toHaveBeenCalled();
    await wrapper.vm.$nextTick();

    expect(players[0].destroy).toHaveBeenCalledOnce();
    expect(videos[1].classes()).toContain('is-active');
    wrapper.unmount();
  });

  it('FLV to HLS uses media readiness when rVFC does not fire', async () => {
    vi.stubGlobal('MediaSource', class FakeMediaSource {});
    const wrapper = mount(GmvPlayerView, { props: { sources: source('http://127.0.0.1/old.flv') } });
    await vi.waitFor(() => expect(players).toHaveLength(1));
    const videos = wrapper.findAll('video');
    videos[0].element.dispatchEvent(new Event('playing'));
    await wrapper.vm.$nextTick();

    const requestVideoFrameCallback = vi.fn(() => 17);
    const cancelVideoFrameCallback = vi.fn();
    Object.defineProperty(videos[1].element, 'requestVideoFrameCallback', {
      configurable: true,
      value: requestVideoFrameCallback,
    });
    Object.defineProperty(videos[1].element, 'cancelVideoFrameCallback', {
      configurable: true,
      value: cancelVideoFrameCallback,
    });
    await wrapper.setProps({ sources: hlsSource('http://127.0.0.1/new.m3u8') });
    await vi.waitFor(() => expect(hlsMock.instances).toHaveLength(1));

    videos[1].element.dispatchEvent(new Event('playing'));
    await wrapper.vm.$nextTick();
    expect(requestVideoFrameCallback).toHaveBeenCalledOnce();
    expect(players[0].destroy).not.toHaveBeenCalled();
    expect(videos[0].classes()).toContain('is-active');

    Object.defineProperty(videos[1].element, 'videoWidth', { configurable: true, value: 1920 });
    Object.defineProperty(videos[1].element, 'readyState', { configurable: true, value: 2 });
    await vi.waitFor(() => expect(players[0].destroy).toHaveBeenCalledOnce());

    expect(cancelVideoFrameCallback).toHaveBeenCalledWith(17);
    expect(videos[1].classes()).toContain('is-active');
    expect(hlsMock.instances[0].loadSource).toHaveBeenCalledWith('http://127.0.0.1/new.m3u8');
    expect(wrapper.emitted('playing')?.at(-1)?.[0]).toMatchObject({
      source: { protocol: 'hls', url: 'http://127.0.0.1/new.m3u8' },
    });
    wrapper.unmount();
  });

  it('active-slot errors enter error state instead of reporting playing', async () => {
    const wrapper = mount(GmvPlayerView, { props: { sources: source('http://127.0.0.1/old.flv') } });
    await vi.waitFor(() => expect(players).toHaveLength(1));
    const videos = wrapper.findAll('video');
    videos[0].element.dispatchEvent(new Event('playing'));
    await wrapper.vm.$nextTick();

    await wrapper.setProps({ sources: source('http://127.0.0.1/new.flv') });
    await vi.waitFor(() => expect(players).toHaveLength(2));
    videos[1].element.dispatchEvent(new Event('playing'));
    await wrapper.vm.$nextTick();
    expect(players[0].destroy).toHaveBeenCalledOnce();

    videos[1].element.dispatchEvent(new ErrorEvent('error', { message: 'new stream failed' }));
    await wrapper.vm.$nextTick();

    expect(players[1].destroy).toHaveBeenCalledOnce();
    expect(wrapper.classes()).toContain('is-error');
    expect(wrapper.classes()).not.toContain('is-playing');
    expect(wrapper.text()).toContain('new stream failed');
    wrapper.unmount();
  });

  it('后端输出准备超过检查点时允许保持当前播放', async () => {
    const wrapper = mount(GmvPlayerView, {
      props: {
        sources: source('http://127.0.0.1/old.flv'),
        outputSwitching: true,
        startupText: '正在生成 HLS 输出',
        startupCanCancel: true,
      },
    });
    await vi.waitFor(() => expect(players).toHaveLength(1));
    wrapper.find('video').element.dispatchEvent(new Event('playing'));
    await wrapper.vm.$nextTick();

    expect(wrapper.text()).toContain('正在生成 HLS 输出');
    await wrapper.get('.startup-switch-banner button').trigger('click');

    expect(wrapper.emitted('playbackSwitchCancel')).toHaveLength(1);
    expect(players[0].destroy).not.toHaveBeenCalled();
    wrapper.unmount();
  });
});
