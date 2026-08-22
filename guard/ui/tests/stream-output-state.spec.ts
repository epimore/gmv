import { expect, test } from '@playwright/test';

test('format switch replaces container metadata and preserves the viewer token', async ({ page }) => {
  await page.goto('/login');

  const { previous, next } = await page.evaluate(async () => {
    const { applyStreamOutputState } = await import('/src/utils/streamOutputState.ts');
    const previous = {
      stream_id: 'stream-1',
      device_id: 'device-1',
      channel_id: 'channel-1',
      node_id: 'stream-node-1',
      instance_id: 'stream-instance-1',
      lease_id: 'lease-1',
      route_id: 'route-1',
      endpoint: '/play/stream-1.flv?gmv-token=viewer-token',
      state: 'running',
      video_codec: 'hev1.1.6.L63',
      audio_codec: 'mp4a.40.2',
      mime_codec: 'video/x-flv',
      output_generation: 1,
    } as const;
    const next = applyStreamOutputState(previous, {
      output_id: 'output-fmp4-1',
      stream_id: 'stream-1',
      output_type: 'fmp4',
      endpoint: '/play/stream-1.fmp4?gmv-token=output-token',
      state: 'ready',
      video_codec: 'hev1.1.6.L63',
      audio_codec: 'mp4a.40.2',
      mime_codec: 'video/mp4; codecs="hev1.1.6.L63, mp4a.40.2"',
      source_audio_state: 'ready',
      output_audio_mode: 'real',
      audio_recovery_eligible: true,
      late_track_watch: true,
      audio_sample_rate: 48_000,
      audio_channels: 1,
      generation: 2,
    });
    return { previous, next };
  });

  expect(next).toMatchObject({
    endpoint: '/play/stream-1.fmp4?gmv-token=viewer-token',
    video_codec: 'hev1.1.6.L63',
    audio_codec: 'mp4a.40.2',
    mime_codec: 'video/mp4; codecs="hev1.1.6.L63, mp4a.40.2"',
    source_audio_state: 'ready',
    output_audio_mode: 'real',
    audio_recovery_eligible: true,
    late_track_watch: true,
    audio_sample_rate: 48_000,
    audio_channels: 1,
    output_generation: 2,
  });
  expect(previous).toMatchObject({
    endpoint: '/play/stream-1.flv?gmv-token=viewer-token',
    mime_codec: 'video/x-flv',
    output_generation: 1,
  });
});
