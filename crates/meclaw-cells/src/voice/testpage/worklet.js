
class PcmFramer extends AudioWorkletProcessor {
  constructor(options) {
    super();
    this.targetRate = options.processorOptions.targetRate;
    this.step = sampleRate / this.targetRate;
    this.frame = Math.round(this.targetRate / 50);
    this.out = new Int16Array(this.frame);
    this.filled = 0;
    this.pos = 0;
    this.tail = new Float32Array(0);
  }
  process(inputs) {
    const chunk = inputs[0] && inputs[0][0];
    if (!chunk) return true;
    const buf = new Float32Array(this.tail.length + chunk.length);
    buf.set(this.tail, 0);
    buf.set(chunk, this.tail.length);
    let p = this.pos;
    while (p + 1 < buf.length) {
      const i = p | 0;
      const frac = p - i;
      let s = buf[i] * (1 - frac) + buf[i + 1] * frac;
      if (s > 1) s = 1; else if (s < -1) s = -1;
      this.out[this.filled++] = s < 0 ? s * 0x8000 : s * 0x7fff;
      if (this.filled === this.frame) {
        const copy = this.out.slice();
        this.port.postMessage(copy.buffer, [copy.buffer]);
        this.filled = 0;
      }
      p += this.step;
    }
    // p overshoots the block by up to one step. Clamping the carry to the
    // block length is what keeps the phase: without it every block restarts
    // near zero, the overhang is swallowed and the output drifts.
    const used = Math.min(p | 0, buf.length);
    this.tail = buf.slice(used);
    this.pos = p - used;
    return true;
  }
}
registerProcessor('pcm-framer', PcmFramer);
