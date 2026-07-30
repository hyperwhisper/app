import { useEffect, useRef, useState, type CSSProperties } from "react";
import { listen } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";
import { db } from "@/lib/audio-level";
import { handOverBrowserSettings } from "@/lib/browser-settings";

const BINS = 64;
const ROWS = 30;
const FILL_START = 8;

// Everything drawn here comes from Rust, measured on the audio the recording
// actually gets. This window must never open the microphone itself: WebKit
// ignores the "raw stream" constraints and puts the device into processed
// mode, and macOS then winds its gain up over the first seconds - the first
// words of every dictation came out nearly inaudible.
type MicLevel = {
  peak: number;
  rms: number;
  seconds: number;
  pitch: number;
  bands: number[];
};

// A ring of bars pointing outwards, their lengths running around the circle in
// a wave. Shown while the model is working, where there is no live sound to
// draw, so the wave is made from three sine waves at different speeds instead:
// no two moments look the same and it never sits still.
//
// Short bars are blue, long bars white, through cyan in between, so the shape
// reads as movement rather than a rotating ring.
function drawWaveRing(ctx: CanvasRenderingContext2D, w: number, h: number, t: number) {
  const BARS = 84;
  const cx = w / 2;
  const cy = h / 2;
  const size = Math.min(w, h);
  const inner = size * 0.14;
  const reach = size * 0.13;

  ctx.clearRect(0, 0, w, h);
  ctx.lineCap = "round";

  // Faint circle the bars stand on, so the shape holds together when most
  // bars are short.
  ctx.beginPath();
  ctx.arc(cx, cy, inner - 3, 0, Math.PI * 2);
  ctx.strokeStyle = "rgba(120, 200, 255, 0.18)";
  ctx.lineWidth = 1;
  ctx.stroke();

  for (let i = 0; i < BARS; i++) {
    const angle = (i / BARS) * Math.PI * 2 + t * 0.22;
    const wave =
      (Math.sin(i * 0.42 - t * 3.1) +
        Math.sin(i * 0.17 + t * 2.0) +
        Math.sin(i * 0.91 + t * 1.3)) /
      3;
    const level = (wave + 1) / 2; // 0 to 1
    const len = inner * 0.12 + reach * level;

    // blue -> cyan -> white as the bar gets longer
    let r: number, g: number, b: number;
    if (level < 0.5) {
      const k = level * 2;
      r = 37 + (56 - 37) * k;
      g = 99 + (189 - 99) * k;
      b = 235 + (248 - 235) * k;
    } else {
      const k = (level - 0.5) * 2;
      r = 56 + (255 - 56) * k;
      g = 189 + (255 - 189) * k;
      b = 248 + (255 - 248) * k;
    }
    const color = `rgb(${r | 0}, ${g | 0}, ${b | 0})`;

    const cos = Math.cos(angle);
    const sin = Math.sin(angle);
    ctx.beginPath();
    ctx.moveTo(cx + cos * inner, cy + sin * inner);
    ctx.lineTo(cx + cos * (inner + len), cy + sin * (inner + len));
    ctx.strokeStyle = color;
    ctx.lineWidth = 2.4;
    ctx.shadowColor = color;
    ctx.shadowBlur = 6 + level * 6;
    ctx.stroke();
  }
  ctx.shadowBlur = 0;
}

// Isometric waterfall spectrogram shown (in its own window) while recording.
// The mic is opened only while the window is active, driven by "indicator-active".
export function Indicator() {
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const waveRef = useRef<HTMLCanvasElement | null>(null);
  const timerRef = useRef<HTMLSpanElement | null>(null);
  const statsRef = useRef<HTMLDivElement | null>(null);
  const textRef = useRef<HTMLDivElement | null>(null);
  // True between the stop and the finished text: the model is running and the
  // whole app is unresponsive, so this has to be obvious.
  const [transcribing, setTranscribing] = useState(false);
  // Something went wrong. Shown here because this is the window that is
  // actually on screen when it happens - the main window is normally hidden.
  const [errorText, setErrorText] = useState<string | null>(null);
  // The drawing loop is set up once and cannot read React state.
  const errorRef = useRef(false);

  // Anything Rust found wrong at startup: a dictation key it could not
  // register, a permission it was not given. This is the only window the user
  // ever sees, so a warning shown anywhere else is a warning nobody reads.
  const [startupWarning, setStartupWarning] = useState<string | null>(null);

  // The line of live numbers is off unless switched on in the tray menu.
  const [showStats, setShowStats] = useState(false);
  // The drawing loop cannot read React state, so it reads this.
  const showStatsRef = useRef(false);

  // The settings the deleted main window kept in browser storage. This window
  // loads on every launch, shown or not, so it is the one that can hand them
  // over without the user having to open anything. Rust copies them once.
  useEffect(() => {
    handOverBrowserSettings().catch((err) =>
      console.error("Could not hand the old settings to Rust:", err)
    );
  }, []);

  // Asked for rather than pushed: Rust finds these before this window exists
  // to hear about them. Rust puts the window on screen once it is told there
  // is something to show.
  useEffect(() => {
    invoke<string[]>("get_startup_warnings")
      .then((warnings) => {
        if (warnings.length === 0) return;
        setStartupWarning(warnings.join(" "));
        invoke("show_startup_warning").catch(() => {});
      })
      .catch(() => {});
  }, []);

  // The ring only draws while the model is working.
  useEffect(() => {
    if (!transcribing) return;
    let raf = 0;
    let start = 0;

    const draw = (now: number) => {
      if (!start) start = now;
      const canvas = waveRef.current;
      if (canvas) {
        const dpr = window.devicePixelRatio || 1;
        const w = canvas.clientWidth;
        const h = canvas.clientHeight;
        if (canvas.width !== w * dpr || canvas.height !== h * dpr) {
          canvas.width = w * dpr;
          canvas.height = h * dpr;
        }
        const ctx = canvas.getContext("2d");
        if (ctx) {
          ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
          drawWaveRing(ctx, w, h, (now - start) / 1000);
        }
      }
      if (timerRef.current) {
        timerRef.current.textContent = `${((now - start) / 1000).toFixed(1)}s`;
      }
      raf = requestAnimationFrame(draw);
    };
    raf = requestAnimationFrame(draw);

    return () => cancelAnimationFrame(raf);
  }, [transcribing]);

  useEffect(() => {
    const apply = (on: boolean) => {
      showStatsRef.current = on;
      setShowStats(on);
    };
    invoke<boolean>("get_debug_stats")
      .then(apply)
      .catch(() => {});
    const unlisten = listen<boolean>("debug-stats-changed", (e) =>
      apply(e.payload)
    );

    return () => {
      unlisten.then((fn) => fn()).catch(() => {});
    };
  }, []);

  useEffect(() => {
    let raf = 0;
    let frame = 0;
    // Newest numbers from the recording itself; empty until the first arrives.
    let mic: MicLevel = { peak: 0, rms: 0, seconds: 0, pitch: 0, bands: [] };

    const history: number[][] = Array.from({ length: ROWS }, () =>
      new Array(BINS).fill(0)
    );

    // One row of the waterfall, taken from the bands Rust sends. Smoothed
    // against the previous row so the surface flows instead of flickering.
    function pushRow() {
      const previous = history[0];
      const row = new Array(BINS).fill(0);
      for (let i = 0; i < BINS; i++) {
        const value = mic.bands[i] ?? 0;
        row[i] = previous[i] * 0.45 + value * 0.55;
      }
      history.pop();
      history.unshift(row);
    }

    // Numbers above the spectrogram. The level and peak come from Rust, so
    // they show what the recording gets, not what this window hears - that is
    // the pair that matters when a dictation comes back empty.
    function updateStats() {
      const el = statsRef.current;
      if (!el || !showStatsRef.current) return;

      let state = "ok";
      let color = "rgba(226, 248, 255, 0.75)";
      if (mic.peak > 0.98) {
        state = "TOO LOUD";
        color = "rgb(255, 138, 128)";
      } else if (mic.rms < 0.005) {
        state = "TOO QUIET";
        color = "rgb(255, 196, 100)";
      }

      // Every field is padded to a fixed width. Without that the numbers
      // change length as you speak and the whole line shifts sideways, which
      // makes it unreadable.
      el.style.color = color;
      el.textContent =
        `${mic.seconds.toFixed(1).padStart(5)}s  ` +
        `rec ${db(mic.rms).padStart(4)} dB  ` +
        `peak ${db(mic.peak).padStart(4)} dB  ` +
        `pitch ${(mic.pitch > 0 ? mic.pitch.toFixed(0) : "--").padStart(3)} Hz  ` +
        state.padEnd(9);
    }

    function draw() {
      const canvas = canvasRef.current;
      if (!canvas) {
        raf = requestAnimationFrame(draw);
        return;
      }
      const ctx = canvas.getContext("2d");
      if (!ctx) {
        // Without this the loop ends for good and the window stays blank.
        raf = requestAnimationFrame(draw);
        return;
      }

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
      // Every frame is unreadable; ~6 times a second is not.
      if (frame % 10 === 0) updateStats();

      const originY = h * 0.9;
      const colW = (w * 0.72) / BINS;
      const stepX = (w * 0.24) / ROWS;
      const stepY = (h * 0.55) / ROWS;
      const heightScale = h * 0.24;

      // Centre the whole shape in the window. Rows recede to the right, so
      // fan them out around the middle row instead of starting every row at
      // the same left edge - otherwise the bright front row sits well left
      // of centre, which is obvious now that there is no panel behind it.
      const rowSpan = colW * (BINS - 1);
      const originX = (w - rowSpan) / 2;
      const skew = (j: number) => (j - (ROWS - 1) / 2) * stepX;

      const px = (i: number, j: number) => originX + i * colW + skew(j);
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

    // Nothing to start or stop any more - the numbers arrive from Rust while
    // it records. Going inactive only clears what is on screen.
    const setActive = (active: boolean) => {
      if (!active) {
        mic = { peak: 0, rms: 0, seconds: 0, pitch: 0, bands: [] };
        setTranscribing(false);
        errorRef.current = false;
        setErrorText(null);
        setStartupWarning(null);
        if (textRef.current) textRef.current.textContent = "";
        for (const row of history) row.fill(0);
      }
    };
    const unlisteners: Promise<() => void>[] = [];
    try {
      unlisteners.push(
        listen<boolean>("indicator-active", (e) => setActive(!!e.payload)),
        listen<MicLevel>("mic-level", (e) => {
          mic = e.payload;
        }),
        // Only the streaming backends send text while you speak; local models
        // send one event at the end, which shows up here for a moment.
        listen<{ text: string }>("transcription", (e) => {
          if (textRef.current) textRef.current.textContent = e.payload.text;
        }),
        listen<string>("transcription-error", (e) => {
          errorRef.current = true;
          setErrorText(e.payload);
          setTranscribing(false);
        }),
        listen("transcription-processing", () => {
          setTranscribing(true);
        }),
        listen("transcription-complete", () => {
          setTranscribing(false);
        }),
        listen<boolean>("mic-level", () => {
          // Sound is arriving again, so the last error is history.
          if (errorRef.current) {
            errorRef.current = false;
            setErrorText(null);
          }
        })
      );
    } catch {
      // events unavailable outside the app: the numbers just stay at zero
    }

    const onVisibility = () => setActive(!document.hidden);
    document.addEventListener("visibilitychange", onVisibility);

    return () => {
      cancelAnimationFrame(raf);
      document.removeEventListener("visibilitychange", onVisibility);
      unlisteners.forEach((u) => u.then((fn) => fn()).catch(() => {}));
    };
  }, []);

  const overlay: CSSProperties = {
    position: "absolute",
    left: 0,
    right: 0,
    textAlign: "center",
    fontFamily:
      "ui-monospace, SFMono-Regular, Menlo, Consolas, monospace",
    fontSize: 10,
    letterSpacing: 0.2,
    whiteSpace: "nowrap",
    pointerEvents: "none",
    // The window has no background, so the text needs its own outline to stay
    // readable over whatever is behind it.
    textShadow: "0 1px 3px rgba(0, 0, 0, 0.9), 0 0 8px rgba(0, 0, 0, 0.7)",
  };

  return (
    <div
      style={{
        position: "relative",
        width: "100vw",
        height: "100vh",
        background: "transparent",
        overflow: "hidden",
      }}
    >
      <canvas
        ref={canvasRef}
        style={{
          width: "100%",
          height: "100%",
          // The spectrogram is frozen while the model runs, so fade it out and
          // let the spinner have the window.
          opacity: transcribing ? 0.15 : 1,
          transition: "opacity 150ms",
        }}
      />
      {/* A dark plate behind the numbers. On a transparent window over light
          content the shadow alone was not enough to read them. */}
      <div
        style={{
          position: "absolute",
          top: 2,
          left: 0,
          right: 0,
          display: showStats ? "flex" : "none",
          justifyContent: "center",
          pointerEvents: "none",
        }}
      >
        <div
          ref={statsRef}
          style={{
            fontFamily: "ui-monospace, SFMono-Regular, Menlo, Consolas, monospace",
            fontSize: 11,
            whiteSpace: "pre",
            padding: "3px 10px",
            borderRadius: 7,
            background: "rgba(10, 14, 20, 0.82)",
            border: "1px solid rgba(255, 255, 255, 0.12)",
          }}
        />
      </div>
      <div
        ref={textRef}
        style={{
          ...overlay,
          bottom: 4,
          fontSize: 11,
          color: "rgba(226, 248, 255, 0.85)",
          overflow: "hidden",
          textOverflow: "ellipsis",
          paddingLeft: 8,
          paddingRight: 8,
        }}
      />
      {(errorText ?? startupWarning) && (
        <div
          className="absolute inset-0 flex items-center justify-center px-4"
          style={{ pointerEvents: "none" }}
        >
          <div
            style={{
              maxHeight: "100%",
              overflowY: "auto",
              padding: "10px 14px",
              borderRadius: 10,
              background: "rgba(24, 10, 10, 0.94)",
              border: "1px solid rgba(255, 138, 128, 0.45)",
              color: "rgb(255, 190, 184)",
              fontFamily: "ui-sans-serif, system-ui, -apple-system, sans-serif",
              fontSize: 12,
              lineHeight: 1.45,
              textAlign: "left",
            }}
          >
            {errorText ?? startupWarning}
          </div>
        </div>
      )}
      {transcribing && !errorText && !startupWarning && (
        <div
          className="absolute inset-0 flex flex-col items-center justify-center gap-3"
          style={{ pointerEvents: "none" }}
        >
          <canvas
            ref={waveRef}
            style={{
              position: "absolute",
              inset: 0,
              width: "100%",
              height: "100%",
            }}
          />
          <div
            style={{
              position: "absolute",
              left: 0,
              right: 0,
              top: "50%",
              marginTop: 62,
              textAlign: "center",
              fontFamily: "ui-sans-serif, system-ui, -apple-system, sans-serif",
              fontSize: 16,
              fontWeight: 600,
              letterSpacing: 0.6,
              color: "rgb(240, 252, 255)",
              textShadow:
                "0 1px 4px rgba(0, 0, 0, 0.95), 0 0 14px rgba(0, 0, 0, 0.8)",
            }}
          >
            Transcribing
            <span className="inline-block animate-pulse">...</span>
            {/* Counts up while the model runs, so a long wait is visibly a
                wait and not a freeze. */}
            <span
              ref={timerRef}
              style={{
                marginLeft: 8,
                fontVariantNumeric: "tabular-nums",
                fontWeight: 500,
                color: "rgba(150, 220, 255, 0.95)",
              }}
            />
          </div>
        </div>
      )}
    </div>
  );
}
