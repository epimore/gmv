import { startGbBroadcast, stopGbBroadcast, type StreamSummary } from '../api/client';

const TARGET_SAMPLE_RATE = 8000;
const FRAME_SAMPLES = 160;

export interface GbBroadcastSession {
  summary: StreamSummary;
  stop: () => Promise<void>;
}

export async function startGbMicrophoneBroadcast(
  deviceId: string,
  scopeId: string,
): Promise<GbBroadcastSession> {
  if (!window.isSecureContext) throw new Error('语音广播需要 HTTPS 安全上下文');
  if (!navigator.mediaDevices?.getUserMedia) throw new Error('当前浏览器不支持麦克风采集');

  const media = await navigator.mediaDevices.getUserMedia({
    audio: { channelCount: 1, echoCancellation: true, noiseSuppression: true },
    video: false,
  });
  let summary: StreamSummary | undefined;
  let socket: WebSocket | undefined;
  let context: AudioContext | undefined;
  let stopped = false;

  const stop = async () => {
    if (stopped) return;
    stopped = true;
    media.getTracks().forEach((track) => track.stop());
    if (socket && socket.readyState < WebSocket.CLOSING) socket.close(1000, 'broadcast stopped');
    if (context && context.state !== 'closed') await context.close();
    if (summary) await stopGbBroadcast(summary.stream_id).catch(() => undefined);
  };

  try {
    summary = await startGbBroadcast(deviceId, {
      request_id: `ui-broadcast-${Date.now()}`,
      channel_id: scopeId,
      talk_codec: 'PCMA',
      talk_sample_rate: 8000,
      talk_channel_count: 1,
      talk_frame_duration_ms: 20,
      trans_mode: 'udp',
    });
    if (!summary.endpoint) throw new Error('广播媒体输入地址为空');

    socket = new WebSocket(new URL(summary.endpoint, window.location.href));
    socket.binaryType = 'arraybuffer';
    await waitForSocket(socket);

    context = new AudioContext();
    const moduleUrl = URL.createObjectURL(new Blob([workletSource()], { type: 'text/javascript' }));
    try {
      await context.audioWorklet.addModule(moduleUrl);
    } finally {
      URL.revokeObjectURL(moduleUrl);
    }
    const source = context.createMediaStreamSource(media);
    const worklet = new AudioWorkletNode(context, 'gmv-broadcast-capture');
    const silent = context.createGain();
    silent.gain.value = 0;
    source.connect(worklet).connect(silent).connect(context.destination);

    const encoder = new PcmaFrameEncoder(context.sampleRate, (frame) => {
      if (socket?.readyState === WebSocket.OPEN) socket.send(frame);
    });
    worklet.port.onmessage = (event: MessageEvent<Float32Array>) => encoder.push(event.data);
    media.getAudioTracks()[0]?.addEventListener('ended', () => void stop(), { once: true });
    socket.addEventListener('close', () => void stop(), { once: true });
    await context.resume();
    return { summary, stop };
  } catch (error) {
    await stop();
    throw error;
  }
}

class PcmaFrameEncoder {
  private samples = new Float32Array(0);
  private position = 0;
  private frame: number[] = [];
  private readonly ratio: number;

  constructor(sourceRate: number, private readonly emit: (frame: Uint8Array) => void) {
    this.ratio = sourceRate / TARGET_SAMPLE_RATE;
  }

  push(input: Float32Array) {
    const combined = new Float32Array(this.samples.length + input.length);
    combined.set(this.samples);
    combined.set(input, this.samples.length);
    while (this.position < combined.length) {
      const index = Math.min(Math.floor(this.position), combined.length - 1);
      this.frame.push(linearToALaw(combined[index]));
      if (this.frame.length === FRAME_SAMPLES) {
        this.emit(Uint8Array.from(this.frame));
        this.frame = [];
      }
      this.position += this.ratio;
    }
    const consumed = Math.min(Math.floor(this.position), combined.length);
    this.samples = combined.slice(consumed);
    this.position -= consumed;
  }
}

function linearToALaw(sample: number): number {
  let pcm = Math.max(-1, Math.min(1, sample));
  let value = Math.round(pcm * 32767);
  const mask = value >= 0 ? 0xd5 : 0x55;
  if (value < 0) value = -value - 1;
  const segments = [0xff, 0x1ff, 0x3ff, 0x7ff, 0xfff, 0x1fff, 0x3fff, 0x7fff];
  let segment = segments.findIndex((limit) => value <= limit);
  if (segment < 0) segment = 7;
  const quantization = segment < 2 ? (value >> 4) & 0x0f : (value >> (segment + 3)) & 0x0f;
  return ((segment << 4) | quantization) ^ mask;
}

function waitForSocket(socket: WebSocket): Promise<void> {
  return new Promise((resolve, reject) => {
    const opened = () => {
      cleanup();
      resolve();
    };
    const failed = () => {
      cleanup();
      reject(new Error('广播媒体连接失败'));
    };
    const cleanup = () => {
      socket.removeEventListener('open', opened);
      socket.removeEventListener('error', failed);
      socket.removeEventListener('close', failed);
    };
    socket.addEventListener('open', opened, { once: true });
    socket.addEventListener('error', failed, { once: true });
    socket.addEventListener('close', failed, { once: true });
  });
}

function workletSource() {
  return `
class GmvBroadcastCapture extends AudioWorkletProcessor {
  process(inputs) {
    const channel = inputs[0] && inputs[0][0];
    if (channel && channel.length) this.port.postMessage(channel.slice());
    return true;
  }
}
registerProcessor('gmv-broadcast-capture', GmvBroadcastCapture);
`;
}
