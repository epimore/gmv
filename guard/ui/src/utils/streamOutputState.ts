import type { StreamOutputSummary, StreamSummary } from '@/api/client';

function preservePlaybackToken(currentEndpoint: string, nextEndpoint: string) {
  const endpoint = nextEndpoint || currentEndpoint;
  const token = currentEndpoint
    .split('?', 2)[1]
    ?.split('&')
    .find((parameter) => parameter.startsWith('gmv-token='));
  if (!endpoint || !token) return endpoint;
  const [base, query = ''] = endpoint.split('?', 2);
  const parameters = query
    .split('&')
    .filter((parameter) => parameter && !parameter.startsWith('gmv-token='));
  parameters.push(token);
  return `${base}?${parameters.join('&')}`;
}

export function applyStreamOutputState(stream: StreamSummary, output: StreamOutputSummary): StreamSummary {
  return {
    ...stream,
    endpoint: preservePlaybackToken(stream.endpoint, output.endpoint),
    video_codec: output.video_codec || stream.video_codec,
    audio_codec: output.audio_codec || stream.audio_codec,
    mime_codec: output.mime_codec || stream.mime_codec,
    source_audio_state: output.source_audio_state,
    output_audio_mode: output.output_audio_mode,
    audio_recovery_eligible: output.audio_recovery_eligible,
    late_track_watch: output.late_track_watch,
    audio_sample_rate: output.audio_sample_rate,
    audio_channels: output.audio_channels,
    output_generation: output.generation,
  };
}
