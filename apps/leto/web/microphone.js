// The page's ear, on the browser's sound thread: it gathers what the
// microphone captures into the pieces Leto sends, and hands each to the
// page. The other end is `apps/leto/src/backend/web/microphone.rs`.

class Gatherer extends AudioWorkletProcessor {
  constructor(options) {
    super();
    // Samples in one piece.
    this.size = options.processorOptions.piece;
    this.piece = new Float32Array(this.size);
    this.filled = 0;
    this.over = false;
    // The page says one thing: stop.
    this.port.onmessage = () => {
      this.over = true;
    };
  }

  process(inputs) {
    // One channel, as the page asked. None while nothing is connected.
    const captured = inputs[0][0];
    if (captured) {
      let at = 0;
      while (at < captured.length) {
        const taken = Math.min(captured.length - at, this.size - this.filled);
        this.piece.set(captured.subarray(at, at + taken), this.filled);
        this.filled += taken;
        at += taken;
        if (this.filled === this.size) {
          // Handed over, not copied: the next piece gets memory of its own.
          this.port.postMessage(this.piece, [this.piece.buffer]);
          this.piece = new Float32Array(this.size);
          this.filled = 0;
        }
      }
    }
    return !this.over;
  }
}

registerProcessor("leto-microphone", Gatherer);
