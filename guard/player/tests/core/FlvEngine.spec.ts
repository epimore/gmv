import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { GmvSource } from "../../src/core/types";

const mpegtsMock = vi.hoisted(() => ({
  createPlayer: vi.fn(),
}));

vi.mock("mpegts.js", () => ({
  default: {
    getFeatureList: () => ({ mseLivePlayback: true }),
    createPlayer: mpegtsMock.createPlayer,
  },
}));

import { GmvPlayerCore } from "../../src/core/GmvPlayerCore";
import { FlvEngine } from "../../src/core/engines/FlvEngine";

function createMpegtsPlayer(play: () => Promise<void> | void = () => Promise.resolve()) {
  return {
    attachMediaElement: vi.fn(),
    load: vi.fn(),
    play: vi.fn(play),
    pause: vi.fn(),
    unload: vi.fn(),
    detachMediaElement: vi.fn(),
    destroy: vi.fn(),
  };
}

function source(hasAudio?: boolean): GmvSource {
  return {
    protocol: "flv",
    url: "http://127.0.0.1/live.flv",
    codec: "h264",
    hasAudio,
  };
}

function testVideo() {
  const video = document.createElement("video");
  video.pause = vi.fn();
  video.load = vi.fn();
  return video;
}

beforeEach(() => {
  mpegtsMock.createPlayer.mockReset();
  mpegtsMock.createPlayer.mockImplementation(() => createMpegtsPlayer());
});

afterEach(() => {
  vi.useRealTimers();
});

describe("FlvEngine audio fallback", () => {
  it.each([undefined, true])("音频状态为 %s 时不强制覆盖媒体流探测结果", async (hasAudio) => {
    const engine = new FlvEngine();
    await engine.attach(testVideo(), source(hasAudio));

    expect(mpegtsMock.createPlayer).toHaveBeenCalledOnce();
    expect(mpegtsMock.createPlayer.mock.calls[0][0]).not.toHaveProperty("hasAudio");
    engine.destroy();
  });

  it("确认无音频时明确使用纯视频模式", async () => {
    const engine = new FlvEngine();
    await engine.attach(testVideo(), source(false));

    expect(mpegtsMock.createPlayer.mock.calls[0][0]).toMatchObject({ hasAudio: false, hasVideo: true });
    engine.destroy();
  });

  it("启动阶段未出现视频画面时自动降级为纯视频模式", async () => {
    vi.useFakeTimers();
    mpegtsMock.createPlayer.mockImplementation(() => createMpegtsPlayer(() => new Promise(() => {})));
    const video = testVideo();
    const core = new GmvPlayerCore({
      video,
      sources: [source(true)],
      autoplay: true,
      muted: true,
    });

    void core.load();
    await vi.advanceTimersByTimeAsync(0);
    expect(mpegtsMock.createPlayer).toHaveBeenCalledTimes(1);

    await vi.advanceTimersByTimeAsync(3_000);
    expect(mpegtsMock.createPlayer).toHaveBeenCalledTimes(2);
    expect(mpegtsMock.createPlayer.mock.calls[1][0]).toMatchObject({ hasAudio: false, hasVideo: true });
    core.destroy();
  });

  it("视频在宽限期内开始播放时保留媒体流的音频探测", async () => {
    vi.useFakeTimers();
    mpegtsMock.createPlayer.mockImplementation(() => createMpegtsPlayer(() => new Promise(() => {})));
    const video = testVideo();
    const core = new GmvPlayerCore({
      video,
      sources: [source(true)],
      autoplay: true,
      muted: true,
    });

    void core.load();
    await vi.advanceTimersByTimeAsync(0);
    video.dispatchEvent(new Event("playing"));
    await vi.advanceTimersByTimeAsync(3_000);

    expect(mpegtsMock.createPlayer).toHaveBeenCalledTimes(1);
    expect(mpegtsMock.createPlayer.mock.calls[0][0]).not.toHaveProperty("hasAudio");
    core.destroy();
  });

  it("纯视频模式仍未启动时结束加载并报告错误", async () => {
    vi.useFakeTimers();
    mpegtsMock.createPlayer.mockImplementation(() => createMpegtsPlayer(() => new Promise(() => {})));
    const core = new GmvPlayerCore({
      video: testVideo(),
      sources: [source(false)],
      autoplay: true,
      muted: true,
    });
    const onError = vi.fn();
    core.on("error", onError);

    void core.load();
    await vi.advanceTimersByTimeAsync(3_000);

    expect(mpegtsMock.createPlayer).toHaveBeenCalledTimes(1);
    expect(onError).toHaveBeenCalledOnce();
    core.destroy();
  });
});
