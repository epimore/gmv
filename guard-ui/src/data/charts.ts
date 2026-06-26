export function lineOption(
  name: string,
  values: number[],
  labels = values.map((_, index) => String(index + 1)),
  color = '#34d8ff',
) {
  const data = values.length ? values : [0];
  const axis = labels.length ? labels : ['当前'];
  return {
    backgroundColor: 'transparent',
    grid: { left: 34, right: 18, top: 24, bottom: 28 },
    tooltip: { trigger: 'axis' },
    xAxis: { type: 'category', data: axis, axisLine: { lineStyle: { color: '#34506d' } } },
    yAxis: { type: 'value', splitLine: { lineStyle: { color: 'rgba(120,220,255,.1)' } } },
    series: [{ name, type: 'line', smooth: true, data, lineStyle: { color, width: 3 }, areaStyle: { color: color + '22' } }],
  };
}

export function graphOption(
  items: Array<{ name: string; value?: number }>,
  links: Array<{ source: string; target: string }>,
) {
  return {
    backgroundColor: 'transparent', tooltip: {},
    series: [{
      type: 'graph', layout: 'force', roam: false, force: { repulsion: 150, edgeLength: 86 },
      label: { show: true, color: '#dff7ff' },
      data: items.map((item) => ({ ...item, symbolSize: 42 + Math.min(item.value ?? 0, 20) })),
      links,
      lineStyle: { color: '#34d8ff', opacity: 0.46, width: 2 },
      itemStyle: { color: '#17345f', borderColor: '#34d8ff', borderWidth: 2 },
    }],
  };
}

export function radarOption(labels: string[], values: number[]) {
  const indicators = labels.map((name) => ({ name, max: 100 }));
  return {
    backgroundColor: 'transparent',
    radar: {
      indicator: indicators.length ? indicators : [{ name: '容量', max: 100 }],
      axisName: { color: '#9edfff' }, splitLine: { lineStyle: { color: 'rgba(120,220,255,.16)' } },
      splitArea: { areaStyle: { color: ['rgba(52,216,255,.03)', 'rgba(168,117,255,.05)'] } },
    },
    series: [{ type: 'radar', data: [{ value: values.length ? values : [0], name: '当前' }], areaStyle: { color: 'rgba(52,216,255,.20)' }, lineStyle: { color: '#34d8ff' } }],
  };
}
