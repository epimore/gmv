import { expect, test } from '@playwright/test';

test('output polling stops for settled outputs and uses the reduced steady interval', async ({ page }) => {
  await page.goto('/login');

  const result = await page.evaluate(async () => {
    const polling = await import('/src/utils/streamOutputPolling.ts');
    const base = {
      streamId: 'stream-1',
      subscriptionId: 'viewer-1',
      outputType: 'flv' as const,
    };
    return {
      unknown: polling.streamOutputNeedsPolling(base),
      settled: polling.streamOutputNeedsPolling({
        ...base,
        outputId: 'out-1',
        outputState: 'ready',
        audioRecoveryEligible: false,
        lateTrackWatch: false,
      }),
      recoverable: polling.streamOutputNeedsPolling({
        ...base,
        outputId: 'out-1',
        outputState: 'ready',
        audioRecoveryEligible: true,
        lateTrackWatch: false,
      }),
      pending: polling.streamOutputNeedsPolling({
        ...base,
        outputId: 'out-1',
        outputState: 'ready',
        audioRecoveryEligible: false,
        lateTrackWatch: false,
        pending: true,
      }),
      terminal: polling.streamOutputNeedsPolling({
        ...base,
        outputId: 'out-1',
        outputState: 'failed',
        audioRecoveryEligible: true,
        lateTrackWatch: true,
      }),
      startupDelay: polling.streamOutputPollDelay(0, 0, 'stream-1'),
      warmupDelay: polling.streamOutputPollDelay(30_000, 0, 'stream-1'),
      steadyDelay: polling.streamOutputPollDelay(61_000, 0, 'stream-1'),
      failureDelay: polling.streamOutputPollDelay(61_000, 3, 'stream-1'),
      distinctEndpointKeys: polling.streamOutputTargetKey({
        ...base,
        endpoint: '/play/stream-1.flv',
      }) !== polling.streamOutputTargetKey({
        ...base,
        endpoint: '/play/stream-2.flv',
      }),
    };
  });

  expect(result).toMatchObject({
    unknown: true,
    settled: false,
    recoverable: true,
    pending: true,
    terminal: false,
    distinctEndpointKeys: true,
  });
  expect(result.startupDelay).toBeGreaterThanOrEqual(900);
  expect(result.startupDelay).toBeLessThanOrEqual(1_100);
  expect(result.warmupDelay).toBeGreaterThanOrEqual(1_800);
  expect(result.warmupDelay).toBeLessThanOrEqual(2_200);
  expect(result.steadyDelay).toBeGreaterThanOrEqual(13_500);
  expect(result.steadyDelay).toBeLessThanOrEqual(16_500);
  expect(result.failureDelay).toBeGreaterThanOrEqual(9_000);
  expect(result.failureDelay).toBeLessThanOrEqual(11_000);
});

test('output matching never falls back to an unrelated ready output', async ({ page }) => {
  await page.goto('/login');

  const result = await page.evaluate(async () => {
    const { matchStreamOutput } = await import('/src/utils/streamOutputPolling.ts');
    const outputs = [
      {
        output_id: 'out-flv-1', stream_id: 'stream-1', output_type: 'flv' as const,
        endpoint: '/play/stream-1.flv', state: 'ready' as const,
      },
      {
        output_id: 'out-fmp4-1', stream_id: 'stream-1', output_type: 'fmp4' as const,
        endpoint: '/play/stream-1.fmp4', state: 'ready' as const,
      },
      {
        output_id: 'out-fmp4-2', stream_id: 'stream-1', output_type: 'fmp4' as const,
        endpoint: '/play/stream-1.fmp4', state: 'ready' as const,
      },
    ];
    const base = { streamId: 'stream-1', subscriptionId: 'viewer-1' };
    return {
      exactId: matchStreamOutput(outputs, { ...base, outputId: 'out-flv-1', outputType: 'flv' })?.output_id,
      exactEndpoint: matchStreamOutput(outputs, {
        ...base,
        endpoint: '/play/stream-1.flv?gmv-token=viewer-token',
        outputType: 'flv',
      })?.output_id,
      ambiguous: matchStreamOutput(outputs, {
        ...base,
        endpoint: '/play/stream-1.fmp4?gmv-token=viewer-token',
        outputType: 'fmp4',
      })?.output_id,
      unrelated: matchStreamOutput(outputs, { ...base, outputType: 'hls' }),
    };
  });

  expect(result).toEqual({
    exactId: 'out-flv-1',
    exactEndpoint: 'out-flv-1',
    ambiguous: undefined,
    unrelated: undefined,
  });
});

test('output list query carries the current subscription id', async ({ page }) => {
  let requestUrl = '';
  await page.route('**/api/v2/streams/stream-1/outputs**', async (route) => {
    requestUrl = route.request().url();
    await route.fulfill({ status: 200, contentType: 'application/json', body: '[]' });
  });
  await page.goto('/login');

  await page.evaluate(async () => {
    const { listStreamOutputs } = await import('/src/api/client.ts');
    await listStreamOutputs('stream-1', 'viewer-1');
  });

  expect(new URL(requestUrl).searchParams.get('subscription_id')).toBe('viewer-1');
});
