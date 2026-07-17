import { mount } from "@vue/test-utils";
import { afterEach, describe, expect, it, vi } from "vitest";
import { nextTick } from "vue";
import type { GmvPlayerControlsConfig, GmvPlayerControlsState } from "../../src/core/types";
import GmvPlayerView from "../../src/view/GmvPlayerView.vue";
import PlayerControls from "../../src/view/PlayerControls.vue";

const playingState: GmvPlayerControlsState = {
  playbackState: "playing",
  audioEnabled: false,
  fullscreen: false,
  infoOpen: false,
  ptzOpen: false,
  recording: false,
  talking: false,
  playbackRate: 1,
  seekMs: 0,
  selectedSourceUrl: "stream-a",
  selectedOutputType: "flv",
};

function mountControls(
  config: GmvPlayerControlsConfig,
  state: GmvPlayerControlsState = playingState,
) {
  return mount(PlayerControls, {
    props: {
      config,
      state,
      capabilities: {
        audio: true,
        ptz: true,
        snapshot: true,
        record: true,
        playback: true,
        talk: true,
        streamSwitch: true,
        presets: true,
      },
      fullscreenSupported: true,
      sources: [
        { protocol: "flv", url: "stream-a", label: "主码流" },
        { protocol: "flv", url: "stream-b", label: "子码流" },
      ],
      outputOptions: [
        { value: "flv", label: "HTTP-FLV" },
        { value: "hls", label: "HLS-fMP4" },
      ],
    },
    attachTo: document.body,
  });
}

afterEach(() => {
  vi.useRealTimers();
  document.body.innerHTML = "";
});

describe("PlayerControls", () => {
  it("未传旧 controlsVisible 时使用新的 controls 配置", () => {
    const wrapper = mount(GmvPlayerView, {
      props: {
        sources: [],
        capabilities: { ptz: false },
        controls: { items: ["play"], visibility: "always" },
      },
      attachTo: document.body,
    });

    expect(wrapper.find('[aria-label="切换播放状态"]').exists()).toBe(true);
    wrapper.unmount();
  });

  it("云台面板默认关闭并由按钮重复点击切换", async () => {
    const wrapper = mount(GmvPlayerView, {
      props: {
        sources: [],
        capabilities: { ptz: true },
        controls: { items: ["ptz"], visibility: "always" },
      },
      attachTo: document.body,
    });
    const ptzButton = wrapper.get('[aria-label="切换云台控制"]');

    expect(ptzButton.attributes("aria-expanded")).toBe("false");
    expect(wrapper.find(".ptz-panel").exists()).toBe(false);

    await ptzButton.trigger("click");
    expect(ptzButton.attributes("aria-expanded")).toBe("true");
    expect(wrapper.find(".ptz-panel").exists()).toBe(true);

    await ptzButton.trigger("click");
    expect(ptzButton.attributes("aria-expanded")).toBe("false");
    expect(wrapper.find(".ptz-panel").exists()).toBe(false);
    wrapper.unmount();
  });

  it("基本信息只在点击信息按钮后展示，并随控件隐藏关闭", async () => {
    const wrapper = mount(GmvPlayerView, {
      props: {
        sources: [],
        deviceId: "device-1",
        channelId: "channel-1",
        title: "东门枪机",
        status: "online",
        viewers: 2,
        mediaMode: "playback",
        playbackStartTimeMs: new Date(2026, 6, 16, 10, 0, 0).getTime(),
        playbackEndTimeMs: new Date(2026, 6, 16, 11, 0, 0).getTime(),
        controls: { items: ["info"], visibility: "always" },
      },
      attachTo: document.body,
    });
    const infoButton = wrapper.get('[aria-label="切换媒体信息"]');

    expect(wrapper.find(".player-topbar").exists()).toBe(false);
    expect(wrapper.find(".media-info-panel").exists()).toBe(false);
    expect(wrapper.find(".osd-item").exists()).toBe(false);
    expect(wrapper.get(".gmv-player").text()).not.toContain("东门枪机");

    await infoButton.trigger("click");
    expect(infoButton.attributes("aria-expanded")).toBe("true");
    expect(wrapper.get(".media-info-panel").text()).toContain("东门枪机");
    expect(wrapper.get(".media-info-panel").text()).toContain("device-1");
    expect(wrapper.get(".media-info-panel").text()).toContain("channel-1");
    expect(wrapper.get(".media-info-panel").text()).toContain("在线");
    expect(wrapper.get(".media-info-panel").text()).toContain("2");
    expect(wrapper.get(".media-info-panel").text()).toContain("历史回放");
    expect(wrapper.get(".media-info-panel").text()).toContain("回放范围");
    expect(wrapper.get(".media-info-panel").text()).toContain("播放倍速1x");

    wrapper.findComponent(PlayerControls).vm.$emit("visibilityChange", false);
    await nextTick();
    expect(wrapper.find(".media-info-panel").exists()).toBe(false);
    expect(infoButton.attributes("aria-expanded")).toBe("false");
    wrapper.unmount();
  });

  it("扩展菜单中的云台按钮可以打开当前画面的云台面板", async () => {
    const wrapper = mount(GmvPlayerView, {
      props: {
        sources: [],
        capabilities: { ptz: true },
        controls: {
          items: ["play", "snapshot", "fullscreen"],
          overflowItems: ["ptz"],
          visibility: "always",
        },
      },
      attachTo: document.body,
    });

    await wrapper.get('[aria-label="更多操作"]').trigger("click");
    await wrapper.get('[aria-label="切换云台控制"]').trigger("click");

    expect(wrapper.find(".overflow-menu").exists()).toBe(false);
    expect(wrapper.find(".ptz-panel").exists()).toBe(true);
    await wrapper.get('[aria-label="更多操作"]').trigger("click");
    expect(wrapper.get('[aria-label="切换云台控制"]').attributes("aria-expanded")).toBe("true");
    wrapper.unmount();
  });

  it("控件超时隐藏会关闭云台且重新显示时不会自动展开", async () => {
    const wrapper = mount(GmvPlayerView, {
      props: {
        sources: [],
        capabilities: { ptz: true },
        controls: { items: ["ptz"], visibility: "auto", autoHideDelayMs: 3000 },
      },
      attachTo: document.body,
    });
    await wrapper.get('[aria-label="切换云台控制"]').trigger("click");
    expect(wrapper.find(".ptz-panel").exists()).toBe(true);

    wrapper.findComponent(PlayerControls).vm.$emit("visibilityChange", false);
    await nextTick();
    expect(wrapper.find(".ptz-panel").exists()).toBe(false);

    wrapper.findComponent(PlayerControls).vm.$emit("visibilityChange", true);
    await nextTick();
    expect(wrapper.get('[aria-label="切换云台控制"]').attributes("aria-expanded")).toBe("false");
    expect(wrapper.find(".ptz-panel").exists()).toBe(false);
    wrapper.unmount();
  });

  it("只有配置了云台能力的播放器才展示云台操作", async () => {
    const withoutPtz = mountControls({ items: ["play"], visibility: "always" });
    const withPtz = mountControls({
      items: ["play"],
      overflowItems: ["ptz"],
      visibility: "always",
    });

    expect(withoutPtz.find('[aria-label="更多操作"]').exists()).toBe(false);
    await withPtz.get('[aria-label="更多操作"]').trigger("click");
    expect(withPtz.find('[aria-label="切换云台控制"]').exists()).toBe(true);
    withoutPtz.unmount();
    withPtz.unmount();
  });

  it("按单路配置分别渲染主操作和竖向扩展操作", async () => {
    const wrapper = mountControls({
      items: ["play", "snapshot", "fullscreen"],
      overflowItems: ["outputType", "info", "audio", "record"],
      visibility: "always",
    });

    expect(wrapper.findAll(".primary-controls > button").map((item) => item.text())).toEqual([
      "暂停",
      "截图",
      "全屏",
      "更多",
    ]);
    expect(wrapper.find('[aria-label="切换声音"]').exists()).toBe(false);
    expect(wrapper.find('[aria-label="媒体输出格式"]').exists()).toBe(false);

    await wrapper.get('[aria-label="更多操作"]').trigger("click");

    expect(wrapper.get(".overflow-menu").isVisible()).toBe(true);
    expect(wrapper.find('.overflow-menu [aria-label="媒体输出格式"]').exists()).toBe(true);
    expect(wrapper.findAll(".overflow-menu > button").map((item) => item.text())).toEqual([
      "信息",
      "声音",
      "录像",
    ]);
    wrapper.unmount();
  });

  it("同一页面的不同播放器配置不会互相覆盖", async () => {
    const snapshotPlayer = mountControls({ items: ["play", "snapshot"], visibility: "always" });
    const audioPlayer = mountControls({
      items: ["play"],
      overflowItems: ["audio"],
      visibility: "always",
    });

    expect(snapshotPlayer.find('[aria-label="截图"]').exists()).toBe(true);
    expect(snapshotPlayer.find('[aria-label="更多操作"]').exists()).toBe(false);
    expect(audioPlayer.find('[aria-label="截图"]').exists()).toBe(false);
    expect(audioPlayer.find('[aria-label="更多操作"]').exists()).toBe(true);

    await audioPlayer.get('[aria-label="更多操作"]').trigger("click");
    expect(snapshotPlayer.find(".overflow-menu").exists()).toBe(false);
    expect(audioPlayer.find('[aria-label="切换声音"]').exists()).toBe(true);
    snapshotPlayer.unmount();
    audioPlayer.unmount();
  });

  it("播放中无操作满 3000ms 后隐藏，活动后重新计时", async () => {
    vi.useFakeTimers();
    const wrapper = mountControls({ items: ["play"], visibility: "auto", autoHideDelayMs: 3000 });

    await vi.advanceTimersByTimeAsync(2999);
    expect(wrapper.get(".control-bar").classes()).not.toContain("is-hidden");

    await vi.advanceTimersByTimeAsync(1);
    expect(wrapper.get(".control-bar").classes()).toContain("is-hidden");

    (wrapper.vm as unknown as { notifyActivity: () => void }).notifyActivity();
    await nextTick();
    expect(wrapper.get(".control-bar").classes()).not.toContain("is-hidden");

    await vi.advanceTimersByTimeAsync(3000);
    expect(wrapper.get(".control-bar").classes()).toContain("is-hidden");
    wrapper.unmount();
  });

  it("回放进度行和功能按钮行使用同一超时隐藏状态", async () => {
    vi.useFakeTimers();
    const wrapper = mountControls(
      { items: ["play", "timeline"], visibility: "auto", autoHideDelayMs: 3000 },
      { ...playingState, durationMs: 10_000 },
    );

    expect(wrapper.find(".playback-timeline-row").exists()).toBe(true);
    expect(wrapper.find(".primary-controls").exists()).toBe(true);

    await vi.advanceTimersByTimeAsync(3000);
    expect(wrapper.get(".control-bar").classes()).toContain("is-hidden");
    expect(wrapper.find(".playback-timeline-row").exists()).toBe(true);
    expect(wrapper.find(".primary-controls").exists()).toBe(true);
    wrapper.unmount();
  });

  it("暂停状态同样按无活动超时隐藏", async () => {
    vi.useFakeTimers();
    const wrapper = mountControls(
      { items: ["play"], visibility: "auto", autoHideDelayMs: 3000 },
      {
        ...playingState,
        playbackState: "idle",
      },
    );

    await vi.advanceTimersByTimeAsync(3000);
    expect(wrapper.get(".control-bar").classes()).toContain("is-hidden");

    (wrapper.vm as unknown as { notifyActivity: () => void }).notifyActivity();
    await nextTick();
    expect(wrapper.get(".control-bar").classes()).not.toContain("is-hidden");
    await vi.advanceTimersByTimeAsync(3000);
    expect(wrapper.get(".control-bar").classes()).toContain("is-hidden");
    wrapper.unmount();
  });

  it("扩展菜单展开后无活动满 3000ms 会与控件一起隐藏", async () => {
    vi.useFakeTimers();
    const wrapper = mountControls({
      items: ["play"],
      overflowItems: ["audio"],
      visibility: "auto",
      autoHideDelayMs: 3000,
    });

    await wrapper.get('[aria-label="更多操作"]').trigger("click");
    await vi.advanceTimersByTimeAsync(2999);
    expect(wrapper.get(".control-bar").classes()).not.toContain("is-hidden");
    expect(wrapper.find(".overflow-menu").exists()).toBe(true);

    await vi.advanceTimersByTimeAsync(1);
    expect(wrapper.get(".control-bar").classes()).toContain("is-hidden");
    expect(wrapper.find(".overflow-menu").exists()).toBe(false);
    wrapper.unmount();
  });

  it("静态悬停和按钮残留焦点不会阻止无活动超时隐藏", async () => {
    vi.useFakeTimers();
    const wrapper = mountControls({ items: ["play"], visibility: "auto", autoHideDelayMs: 3000 });
    const controls = wrapper.get(".control-bar");

    await controls.trigger("pointerenter");
    await wrapper.get("button").trigger("focusin");
    await vi.advanceTimersByTimeAsync(2999);
    expect(controls.classes()).not.toContain("is-hidden");

    await vi.advanceTimersByTimeAsync(1);
    expect(controls.classes()).toContain("is-hidden");

    (wrapper.vm as unknown as { notifyActivity: () => void }).notifyActivity();
    expect(vi.getTimerCount()).toBeGreaterThan(0);
    wrapper.unmount();
    expect(vi.getTimerCount()).toBe(0);
  });

  it("鼠标离开播放窗口后会释放交互状态并恢复超时隐藏", async () => {
    vi.useFakeTimers();
    const wrapper = mountControls({
      items: ["play"],
      overflowItems: ["audio"],
      visibility: "auto",
      autoHideDelayMs: 3000,
    });
    const controls = wrapper.get(".control-bar");

    await controls.trigger("pointerenter");
    await wrapper.get('[aria-label="更多操作"]').trigger("click");
    await wrapper.get("button").trigger("focusin");
    (wrapper.vm as unknown as { setExternalInteractionActive: (active: boolean) => void })
      .setExternalInteractionActive(true);
    await vi.advanceTimersByTimeAsync(6000);
    expect(controls.classes()).not.toContain("is-hidden");
    expect(wrapper.find(".overflow-menu").exists()).toBe(true);

    (wrapper.vm as unknown as { notifySurfaceLeave: () => void }).notifySurfaceLeave();
    await nextTick();
    expect(wrapper.find(".overflow-menu").exists()).toBe(false);

    await vi.advanceTimersByTimeAsync(3000);
    expect(controls.classes()).toContain("is-hidden");
    wrapper.unmount();
  });

  it("播放器根节点离开事件会传递给控件组件", async () => {
    const wrapper = mount(GmvPlayerView, {
      props: {
        sources: [],
        controls: {
          items: ["play"],
          overflowItems: ["audio"],
          visibility: "always",
        },
      },
      attachTo: document.body,
    });

    await wrapper.get('[aria-label="更多操作"]').trigger("click");
    expect(wrapper.find(".overflow-menu").exists()).toBe(true);

    await wrapper.get(".gmv-player").trigger("pointerleave");
    expect(wrapper.find(".overflow-menu").exists()).toBe(false);
    wrapper.unmount();
  });

  it("发送类型化 action 并携带选择值", async () => {
    const wrapper = mountControls({
      items: ["snapshot", "outputType", "info", "playbackRate"],
      visibility: "always",
      playbackRates: [1, 2],
    });

    await wrapper.get('[aria-label="截图"]').trigger("click");
    await wrapper.get('[aria-label="媒体输出格式"]').setValue("hls");
    await wrapper.get('[aria-label="切换媒体信息"]').trigger("click");
    await wrapper.get('[aria-label="播放倍速"]').setValue("2");

    expect(wrapper.emitted("action")).toEqual([
      [{ type: "snapshot" }],
      [{ type: "output-type-change", outputType: "hls" }],
      [{ type: "info-toggle" }],
      [{ type: "rate-change", rate: 2 }],
    ]);
    wrapper.unmount();
  });

  it("回放时间轴展示首尾时刻、跨度刻度和悬停时刻", async () => {
    const start = new Date(2026, 6, 16, 10, 0, 0).getTime();
    const end = new Date(2026, 6, 16, 11, 0, 0).getTime();
    const wrapper = mountControls(
      {
        items: ["play", "snapshot", "fullscreen", "timeline"],
        overflowItems: ["info", "playbackRate"],
        visibility: "always",
      },
      {
        ...playingState,
        durationMs: end - start,
        timelineStartTimeMs: start,
        timelineEndTimeMs: end,
      },
    );

    expect(wrapper.find(".playback-timeline-row .timeline").exists()).toBe(true);
    expect(wrapper.find(".primary-controls .timeline").exists()).toBe(false);
    expect(wrapper.find('.primary-controls [aria-label="切换播放状态"]').exists()).toBe(true);
    expect(wrapper.findAll(".primary-controls > button").map((item) => item.text())).toEqual([
      "暂停",
      "截图",
      "全屏",
      "更多",
    ]);
    expect(wrapper.find(".timeline-jump").exists()).toBe(false);

    await wrapper.get('[aria-label="更多操作"]').trigger("click");
    expect(wrapper.find('.overflow-menu [aria-label="播放倍速"]').exists()).toBe(true);
    expect(wrapper.find(".overflow-menu .overflow-timeline-jump").exists()).toBe(true);

    expect(wrapper.text()).toContain("07-16 10:00:00");
    expect(wrapper.text()).toContain("10:15:00");
    expect(wrapper.text()).toContain("10:30:00");
    expect(wrapper.text()).toContain("10:45:00");
    expect(wrapper.text()).toContain("07-16 11:00:00");

    const range = wrapper.get<HTMLInputElement>('[aria-label="回放进度"]');
    Object.defineProperty(range.element, "getBoundingClientRect", {
      value: () => ({ left: 0, width: 100 }),
    });
    range.element.dispatchEvent(new MouseEvent("pointermove", { bubbles: true, clientX: 50 }));
    await nextTick();

    expect(wrapper.text()).toContain("2026-07-16 10:30:00");
    wrapper.unmount();
  });

  it("按输入秒数前后跳跃并限制在所选时段内", async () => {
    const wrapper = mountControls(
      { items: ["timeline"], visibility: "always" },
      { ...playingState, seekMs: 5_000, durationMs: 10_000 },
    );
    await wrapper.get('[aria-label="更多操作"]').trigger("click");
    await wrapper.get('[aria-label="跳跃秒数"]').setValue("3");
    await wrapper.get('[aria-label="向前跳跃"]').trigger("click");
    await wrapper.get('[aria-label="向后跳跃"]').trigger("click");

    expect(wrapper.emitted("action")).toEqual([
      [{ type: "seek", timeMs: 8_000 }],
      [{ type: "seek", timeMs: 2_000 }],
    ]);

    await wrapper.get('[aria-label="跳跃秒数"]').setValue("30");
    await wrapper.get('[aria-label="向前跳跃"]').trigger("click");
    await wrapper.get('[aria-label="向后跳跃"]').trigger("click");
    expect(wrapper.get<HTMLInputElement>('[aria-label="跳跃秒数"]').element.value).toBe("10");
    expect(wrapper.emitted("action")?.slice(-2)).toEqual([
      [{ type: "seek", timeMs: 10_000 }],
      [{ type: "seek", timeMs: 0 }],
    ]);
    wrapper.unmount();
  });
});
