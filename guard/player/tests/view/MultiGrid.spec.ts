import { mount } from "@vue/test-utils";
import { describe, expect, it, vi } from "vitest";
import MultiGrid from "../../src/view/MultiGrid.vue";

vi.mock("mpegts.js", () => ({
  default: {
    getFeatureList: () => ({ mseLivePlayback: true }),
    createPlayer: () => ({
      attachMediaElement: vi.fn(),
      load: vi.fn(),
      play: vi.fn(() => Promise.resolve()),
      pause: vi.fn(),
      unload: vi.fn(),
      detachMediaElement: vi.fn(),
      destroy: vi.fn(),
    }),
  },
}));

describe("MultiGrid output selector", () => {
  it("在同一工具栏展示概要、宫格切换和外部操作", () => {
    const wrapper = mount(MultiGrid, {
      props: { gridSize: 4, cells: [] },
      slots: {
        summary: '<span class="test-summary">多画面播放 · 实时直播 · 运行中 1 路</span>',
        actions: '<button class="test-fullscreen">满屏</button>',
      },
    });

    const toolbar = wrapper.get(".grid-toolbar");
    expect(wrapper.findAll(".grid-toolbar")).toHaveLength(1);
    expect(toolbar.text()).toContain("多画面播放 · 实时直播 · 运行中 1 路");
    expect(toolbar.text()).toContain("多宫格");
    expect(toolbar.text()).toContain("满屏");
    expect(toolbar.find(".grid-body").exists()).toBe(false);
    wrapper.unmount();
  });

  it("keeps media output selection scoped to the selected cell", async () => {
    const wrapper = mount(MultiGrid, {
      props: {
        gridSize: 4,
        cells: [
          {
            title: "camera-a",
            sources: [{ protocol: "flv", url: "stream-a.flv" }],
            controls: { items: ["outputType"], visibility: "always" },
            outputType: "flv",
            outputOptions: [
              { value: "flv", label: "HTTP-FLV" },
              { value: "fmp4", label: "HTTP-fMP4" },
              { value: "hls", label: "HLS-fMP4" },
            ],
          },
          {
            title: "camera-b",
            sources: [{ protocol: "flv", url: "stream-b.flv" }],
            controls: { items: ["outputType"], visibility: "always" },
            outputType: "flv",
            outputOptions: [
              { value: "flv", label: "HTTP-FLV" },
              { value: "fmp4", label: "HTTP-fMP4" },
              { value: "hls", label: "HLS-fMP4" },
            ],
          },
        ],
      },
    });

    const selectors = wrapper.findAll('[aria-label="媒体输出格式"]');
    await selectors[1].setValue("hls");
    await selectors[0].setValue("fmp4");

    expect(wrapper.emitted("outputTypeChange")).toEqual([
      [{ index: 1, outputType: "hls" }],
      [{ index: 0, outputType: "fmp4" }],
    ]);
    wrapper.unmount();
  });

  it("forwards playback controls and progress with the cell index", async () => {
    const wrapper = mount(MultiGrid, {
      props: {
        gridSize: 1,
        cells: [{
          title: "playback-a",
          mediaMode: "playback",
          sources: [{ protocol: "fmp4", url: "playback-a.fmp4", rateMode: "remote-stream" }],
          playbackDurationMs: 60_000,
          playbackStartTimeMs: 1_000,
          playbackEndTimeMs: 61_000,
          capabilities: { playback: true },
          controls: { items: ["play", "timeline"], overflowItems: ["playbackRate"] },
        }],
      },
    });
    const player = wrapper.findComponent({ name: "GmvPlayerView" });

    player.vm.$emit("playbackRateChange", { rate: 2 });
    player.vm.$emit("playbackStateChange", { paused: true });
    player.vm.$emit("playbackProgress", { mediaTimeMs: 12_000 });
    await wrapper.vm.$nextTick();

    expect(wrapper.emitted("playbackRateChange")).toEqual([[{ index: 0, payload: { rate: 2 } }]]);
    expect(wrapper.emitted("playbackStateChange")).toEqual([[{ index: 0, payload: { paused: true } }]]);
    expect(wrapper.emitted("playbackProgress")).toEqual([[{ index: 0, payload: { mediaTimeMs: 12_000 } }]]);
    wrapper.unmount();
  });
});
