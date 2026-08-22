import type { StreamOutputSummary } from '@/api/client';

export interface StreamOutputPollTarget {
  streamId: string;
  subscriptionId?: string;
  outputId?: string;
  endpoint?: string;
  outputType: StreamOutputSummary['output_type'];
  outputState?: StreamOutputSummary['state'];
  audioRecoveryEligible?: boolean;
  lateTrackWatch?: boolean;
  generation?: number;
  pending?: boolean;
}

export function streamOutputQueryKey(target: Pick<StreamOutputPollTarget, 'streamId' | 'subscriptionId'>) {
  return `${target.streamId}\u0000${target.subscriptionId || ''}`;
}

export function streamOutputTargetKey(target: StreamOutputPollTarget) {
  return [
    streamOutputQueryKey(target),
    target.outputId || '',
    endpointPath(target.endpoint),
    target.outputType,
    target.outputState || '',
    target.audioRecoveryEligible === true ? 'recover' : target.audioRecoveryEligible === false ? 'terminal' : 'unknown',
    target.lateTrackWatch === true ? 'watch' : target.lateTrackWatch === false ? 'settled' : 'unknown',
    String(target.generation || 0),
    target.pending ? 'pending' : 'committed',
  ].join('\u0001');
}

export function streamOutputNeedsPolling(target: StreamOutputPollTarget) {
  if (target.pending) return true;
  if (target.outputState === 'closed' || target.outputState === 'failed') return false;
  if (!target.outputId || target.outputState === 'preparing' || !target.outputState) return true;
  return target.audioRecoveryEligible !== false || target.lateTrackWatch !== false;
}

export function streamOutputPollDelay(ageMs: number, failures: number, stableKey: string) {
  const failureBackoff = [2_000, 5_000, 10_000];
  const base = failures > 0
    ? failureBackoff[Math.min(failures, failureBackoff.length) - 1]
    : ageMs < 10_000 ? 1_000 : ageMs < 60_000 ? 2_000 : 15_000;
  let hash = 0;
  for (const char of stableKey) hash = (hash * 31 + char.charCodeAt(0)) | 0;
  return Math.round(base * (0.9 + (Math.abs(hash) % 201) / 1_000));
}

function endpointPath(endpoint?: string) {
  return (endpoint || '').split('?', 1)[0];
}

function uniqueOutput(outputs: StreamOutputSummary[]) {
  return outputs.length === 1 ? outputs[0] : undefined;
}

export function matchStreamOutput(outputs: StreamOutputSummary[], target: StreamOutputPollTarget) {
  if (target.outputId) {
    const exact = outputs.find((output) => output.output_id === target.outputId);
    if (exact) return exact;
  }

  const targetEndpoint = endpointPath(target.endpoint);
  if (targetEndpoint) {
    const endpointMatches = outputs.filter((output) => endpointPath(output.endpoint) === targetEndpoint);
    const typedEndpoint = uniqueOutput(endpointMatches.filter((output) => output.output_type === target.outputType));
    if (typedEndpoint) return typedEndpoint;
    const endpointMatch = uniqueOutput(endpointMatches);
    if (endpointMatch) return endpointMatch;
  }

  return uniqueOutput(outputs.filter((output) => output.output_type === target.outputType));
}
