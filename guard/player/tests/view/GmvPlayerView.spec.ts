import { mount } from '@vue/test-utils';
import { beforeEach, describe, expect, it, vi } from 'vitest';

const players: Array<ReturnType<typeof createMpegtsPlayer>> = [];
const mpegtsMock = vi.hoisted(() => ({ createPlayer: vi.fn() }));

vi.mock('mpegts.js', () => ({
  default: {
    getFeatureList: () => ({ mseLivePlayback: true }),
    createPlayer: mpegtsMock.createPlayer,
  },
}));

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

beforeEach(() => {
  players.length = 0;
  mpegtsMock.createPlayer.mockReset();
  mpegtsMock.createPlayer.mockImplementation(() => {
    const player = createMpegtsPlayer();
    players.push(player);
    return player;
  });
});

describe('GmvPlayerView make-before-break', () => {
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
    await wrapper.vm.$nextTick();

    expect(players[0].destroy).toHaveBeenCalledOnce();
    expect(videos[1].classes()).toContain('is-active');
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
