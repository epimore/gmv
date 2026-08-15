import { mount } from "@vue/test-utils";
import { beforeEach, describe, expect, it, vi } from "vitest";
import MultiGrid from "../../src/view/MultiGrid.vue";
import GmvPlayerView from "../../src/view/GmvPlayerView.vue";

const mpegtsMock = vi.hoisted(() => ({ createPlayer: vi.fn() }));

vi.mock("mpegts.js", () => ({
  default: {
    getFeatureList: () => ({ mseLivePlayback: true }),
    createPlayer: mpegtsMock.createPlayer,
  },
}));

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

describe("MultiGrid output selector", () => {
  beforeEach(() => {
    mpegtsMock.createPlayer.mockReset();
    mpegtsMock.createPlayer.mockImplementation(createMpegtsPlayer);
  });

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

  it("keeps live and playback output selection scoped to the selected cell", async () => {
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
            mediaMode: "playback",
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

  it("keeps manual control visibility isolated per player cell", async () => {
    const wrapper = mount(MultiGrid, {
      props: {
        gridSize: 4,
        cells: [
          {
            title: "live-a",
            mediaMode: "live",
            sources: [{ protocol: "flv", url: "stream-a.flv" }],
            controls: { items: ["play"], visibility: "auto", autoHideDelayMs: 3000 },
          },
          {
            title: "playback-b",
            mediaMode: "playback",
            sources: [{ protocol: "flv", url: "stream-b.flv" }],
            controls: { items: ["play", "timeline"], visibility: "auto", autoHideDelayMs: 3000 },
          },
        ],
      },
    });
    const players = wrapper.findAllComponents(GmvPlayerView);

    await players[0].findAll(".gmv-video")[0].trigger("click");

    expect(players[0].get(".gmv-player").classes()).toContain("player-chrome-hidden");
    expect(players[1].get(".gmv-player").classes()).not.toContain("player-chrome-hidden");
    expect(players[1].find(".playback-timeline-row").exists()).toBe(true);

    await players[0].findAll(".gmv-video")[0].trigger("click");
    expect(players[0].get(".gmv-player").classes()).not.toContain("player-chrome-hidden");
    expect(players[1].get(".gmv-player").classes()).not.toContain("player-chrome-hidden");
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
          mediaTransport: "TCP 被动",
          capabilities: { playback: true },
          controls: { items: ["play", "timeline"], overflowItems: ["playbackRate"] },
        }],
      },
    });
    const player = wrapper.findComponent({ name: "GmvPlayerView" });

    player.vm.$emit("playbackRateChange", { rate: 2 });
    player.vm.$emit("playbackStateChange", { paused: true });
    player.vm.$emit("playbackProgress", { mediaTimeMs: 12_000 });
    player.vm.$emit("cloudRecordCreate", { startTimeMs: 10_000, endTimeMs: 130_000 });
    await wrapper.vm.$nextTick();

    (wrapper.vm as unknown as { confirmPlaybackProgress: (index: number, timeMs: number) => void })
      .confirmPlaybackProgress(0, 12_000);
    await wrapper.vm.$nextTick();

    expect(wrapper.emitted("playbackRateChange")).toEqual([[{ index: 0, payload: { rate: 2 } }]]);
    expect(wrapper.emitted("playbackStateChange")).toEqual([[{ index: 0, payload: { paused: true } }]]);
    expect(wrapper.emitted("playbackProgress")).toEqual([[{ index: 0, payload: { mediaTimeMs: 12_000 } }]]);
    expect(wrapper.emitted("cloudRecordCreate")).toEqual([[{ index: 0, payload: { startTimeMs: 10_000, endTimeMs: 130_000 } }]]);
    expect(player.props("mediaTransport")).toBe("TCP 被动");
    expect((wrapper.get('[aria-label="回放进度"]').element as HTMLInputElement).value).toBe("12000");
    wrapper.unmount();
  });

  it("reorders stable cells without reconnecting their media sources", async () => {
    const cameraA = {
      cellId: "session-a:device-a:channel-a",
      title: "camera-a",
      deviceId: "device-a",
      channelId: "channel-a",
      sources: [{ protocol: "flv" as const, url: "stream-a.flv" }],
    };
    const cameraB = {
      cellId: "session-b:device-b:channel-b",
      title: "camera-b",
      deviceId: "device-b",
      channelId: "channel-b",
      sources: [{ protocol: "flv" as const, url: "stream-b.flv" }],
    };
    const wrapper = mount(MultiGrid, {
      props: { gridSize: 4, cells: [cameraA, cameraB] },
    });
    await vi.waitFor(() => expect(mpegtsMock.createPlayer).toHaveBeenCalledTimes(2));
    const playerInstances = wrapper.findAllComponents(GmvPlayerView).map((player) => player.vm.$.uid);

    await wrapper.setProps({ cells: [cameraB, cameraA] });

    const reorderedPlayers = wrapper.findAllComponents(GmvPlayerView);
    expect(reorderedPlayers.map((player) => player.props("title"))).toEqual(["camera-b", "camera-a"]);
    expect(reorderedPlayers.map((player) => player.vm.$.uid)).toEqual([playerInstances[1], playerInstances[0]]);
    expect(mpegtsMock.createPlayer).toHaveBeenCalledTimes(2);
    wrapper.unmount();
  });

  it("keeps players mounted while changing the visible page", async () => {
    const cells = [
      {
        cellId: "session-a:device-a:channel-a",
        title: "camera-a",
        sources: [{ protocol: "flv" as const, url: "stream-a.flv" }],
      },
      {
        cellId: "session-b:device-b:channel-b",
        title: "camera-b",
        sources: [{ protocol: "flv" as const, url: "stream-b.flv" }],
      },
    ];
    const wrapper = mount(MultiGrid, {
      props: { gridSize: 1, visibleStart: 0, cells },
    });
    await vi.waitFor(() => expect(mpegtsMock.createPlayer).toHaveBeenCalledTimes(2));
    const playerInstances = wrapper.findAllComponents(GmvPlayerView).map((player) => player.vm.$.uid);

    await wrapper.setProps({ visibleStart: 1 });

    const gridCells = wrapper.findAll(".grid-cell");
    expect(gridCells[0].attributes("style")).toContain("display: none");
    expect(gridCells[1].attributes("style") || "").not.toContain("display: none");
    expect(wrapper.findAllComponents(GmvPlayerView).map((player) => player.vm.$.uid)).toEqual(playerInstances);
    expect(mpegtsMock.createPlayer).toHaveBeenCalledTimes(2);
    await gridCells[1].get('[aria-label="关闭画面"]').trigger("click");
    expect(wrapper.emitted("close")).toEqual([[{ index: 0 }]]);
    wrapper.unmount();
  });
});
