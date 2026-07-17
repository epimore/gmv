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
});
