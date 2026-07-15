import { mount } from "@vue/test-utils";
import { describe, expect, it } from "vitest";
import MultiGrid from "../../src/view/MultiGrid.vue";

describe("MultiGrid output selector", () => {
  it("keeps media output selection scoped to the selected cell", async () => {
    const wrapper = mount(MultiGrid, {
      props: {
        gridSize: 4,
        cells: [
          {
            title: "camera-a",
            sources: [],
            outputType: "flv",
            outputOptions: [
              { value: "flv", label: "HTTP-FLV" },
              { value: "fmp4", label: "HTTP-fMP4" },
              { value: "hls", label: "HLS-fMP4" },
            ],
          },
          {
            title: "camera-b",
            sources: [],
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

    await wrapper.findAll(".grid-cell-output select")[1].setValue("hls");
    await wrapper.findAll(".grid-cell-output select")[0].setValue("fmp4");

    expect(wrapper.emitted("outputTypeChange")).toEqual([
      [{ index: 1, outputType: "hls" }],
      [{ index: 0, outputType: "fmp4" }],
    ]);
  });
});
