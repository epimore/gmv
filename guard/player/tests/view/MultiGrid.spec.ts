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
              { value: "hls", label: "HLS-fMP4" },
            ],
          },
          {
            title: "camera-b",
            sources: [],
            outputType: "flv",
            outputOptions: [
              { value: "flv", label: "HTTP-FLV" },
              { value: "hls", label: "HLS-fMP4" },
            ],
          },
        ],
      },
    });

    await wrapper.findAll(".grid-cell-output select")[1].setValue("hls");

    expect(wrapper.emitted("outputTypeChange")).toEqual([
      [{ index: 1, outputType: "hls" }],
    ]);
  });
});
