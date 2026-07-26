import { useEffect, useRef } from "react";
import { listen } from "@tauri-apps/api/event";

const BINS = 64;
const ROWS = 30;
const FILL_START = 8;

// Isometric waterfall spectrogram shown (in its own window) while recording.
// The mic is opened only while the window is active, driven by "indicator-active".
export function Indicator() {
  const canvasRef = useRef<HTMLCanvasElement | null>(null);

  useEffect(() => {
    let audioCtx: AudioContext | null = null;
    let analyser: AnalyserNode | null = null;
    let source: MediaStreamAudioSourceNode | null = null;
    let stream: MediaStream | null = null;
    let raf = 0;
    let running = false;
    let disposed = false;
    let frame = 0;

    const history: number[][] = Array.from({ length: ROWS }, () =>
      new Array(BINS).fill(0)
    );

    async function startAudio() {
      if (running || disposed) return;
      running = true;
      try {
        stream = await navigator.mediaDevices.getUserMedia({ audio: true });
        if (disposed) {
          stream.getTracks().forEach((t) => t.stop());
          return;
        }
        audioCtx = new AudioContext();
        analyser = audioCtx.createAnalyser();
        analyser.fftSize = 2048;
        analyser.smoothingTimeConstant = 0.7;
        source = audioCtx.createMediaStreamSource(stream);
        source.connect(analyser);
      } catch {
        // mic unavailable: surface stays flat
      }
    }

    function stopAudio() {
      running = false;
      source?.disconnect();
      analyser?.disconnect();
      audioCtx?.close().catch(() => {});
      stream?.getTracks().forEach((t) => t.stop());
      source = analyser = audioCtx = stream = null;
    }

    function pushRow() {
      const row = new Array(BINS).fill(0);
      if (analyser) {
        const bins = analyser.frequencyBinCount;
        const data = new Uint8Array(bins);
        analyser.getByteFrequencyData(data);
        // log-spaced frequencies so the voice range fills the width
        const minBin = 2;
        const maxBin = Math.max(minBin + 1, Math.floor(bins * 0.5));
        for (let i = 0; i < BINS; i++) {
          const src = Math.round(minBin * Math.pow(maxBin / minBin, i / (BINS - 1)));
          row[i] = data[Math.min(bins - 1, src)] / 255;
        }
      }
      history.pop();
      history.unshift(row);
    }

    function draw() {
      const canvas = canvasRef.current;
      if (!canvas) {
        raf = requestAnimationFrame(draw);
        return;
      }
      const ctx = canvas.getContext("2d");
      if (!ctx) return;

      const dpr = window.devicePixelRatio || 1;
      const w = canvas.clientWidth;
      const h = canvas.clientHeight;
      if (canvas.width !== w * dpr || canvas.height !== h * dpr) {
        canvas.width = w * dpr;
        canvas.height = h * dpr;
      }
      ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
      ctx.clearRect(0, 0, w, h);

      frame++;
      if (frame % 2 === 0) pushRow();

      const originX = w * 0.04;
      const originY = h * 0.9;
      const colW = (w * 0.72) / BINS;
      const stepX = (w * 0.24) / ROWS;
      const stepY = (h * 0.55) / ROWS;
      const heightScale = h * 0.24;

      const px = (i: number, j: number) => originX + i * colW + j * stepX;
      const py = (j: number, mag: number) => originY - j * stepY - mag * heightScale;

      const crestPath = (j: number) => {
        const row = history[j];
        const p = new Path2D();
        p.moveTo(px(0, j), py(j, row[0]));
        for (let i = 1; i < BINS; i++) p.lineTo(px(i, j), py(j, row[i]));
        return p;
      };

      for (let j = ROWS - 1; j >= 0; j--) {
        const row = history[j];
        const depth = 1 - j / ROWS;
        const baselineY = originY - j * stepY;
        const crest = crestPath(j);

        if (j >= FILL_START) {
          const body = new Path2D(crest);
          body.lineTo(px(BINS - 1, j), baselineY);
          body.lineTo(px(0, j), baselineY);
          body.closePath();
          const t = (j - FILL_START) / (ROWS - 1 - FILL_START);
          const r = Math.round(56 + (236 - 56) * t);
          const g = Math.round(132 + (248 - 132) * t);
          const b = Math.round(250 + (255 - 250) * t);
          ctx.globalAlpha = 0.32 + t * 0.3;
          ctx.fillStyle = `rgb(${r}, ${g}, ${b})`;
          ctx.fill(body);
        }

        ctx.globalAlpha = 0.1 + depth * 0.18;
        ctx.strokeStyle = "rgb(90, 205, 255)";
        ctx.lineWidth = 4;
        ctx.stroke(crest);

        ctx.globalAlpha = 0.45 + depth * 0.55;
        ctx.strokeStyle = "rgb(226, 248, 255)";
        ctx.lineWidth = 1.1;
        ctx.stroke(crest);

        ctx.globalAlpha = 0.3 + depth * 0.6;
        ctx.strokeStyle = "rgb(240, 252, 255)";
        ctx.lineWidth = 1.4;
        const capW = colW * 1.6;
        ctx.beginPath();
        for (let i = 2; i < BINS - 2; i++) {
          const m = row[i];
          if (m > 0.32 && m >= row[i - 1] && m > row[i + 1]) {
            const cx = px(i, j);
            const cy = py(j, m);
            ctx.moveTo(cx - capW / 2, cy - 4);
            ctx.lineTo(cx + capW / 2, cy - 4);
          }
        }
        ctx.stroke();
      }
      ctx.globalAlpha = 1;

      raf = requestAnimationFrame(draw);
    }

    draw();

    const setActive = (active: boolean) => (active ? startAudio() : stopAudio());
    let unlisten: Promise<() => void> | null = null;
    try {
      unlisten = listen<boolean>("indicator-active", (e) => setActive(!!e.payload));
    } catch {
      unlisten = null;
    }

    const onVisibility = () => setActive(!document.hidden);
    document.addEventListener("visibilitychange", onVisibility);
    if (!document.hidden) startAudio();

    return () => {
      disposed = true;
      cancelAnimationFrame(raf);
      document.removeEventListener("visibilitychange", onVisibility);
      unlisten?.then((fn) => fn()).catch(() => {});
      stopAudio();
    };
  }, []);

  return (
    <div
      style={{
        width: "100vw",
        height: "100vh",
        background: "rgba(8, 10, 20, 0.72)",
        backdropFilter: "blur(18px)",
        WebkitBackdropFilter: "blur(18px)",
        borderRadius: 18,
        overflow: "hidden",
      }}
    >
      <canvas ref={canvasRef} style={{ width: "100%", height: "100%" }} />
    </div>
  );
}
