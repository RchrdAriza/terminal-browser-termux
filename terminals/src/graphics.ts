import fs from "node:fs";

import type { Terminal } from "./terminal";

export type GraphicsSupport = "supported" | "unsupported" | "unknown";

export const SKIP_ENV = "TERMINAL_BROWSER_SKIP_GRAPHICS_CHECK";
export const BACKEND_ENV = "TERMINAL_BROWSER_BACKEND";

export type GraphicsBackend = "kitty" | "sixel" | "iterm";

const PROBE_ID = 4207;

const PROBE_TIMEOUT_MS = 500;

function passthrough(seq: string): string {
  return `\x1bPtmux;${seq.replaceAll("\x1b", "\x1b\x1b")}\x1b\\`;
}

function probeSequence(wrap: boolean): string {
  const query = `\x1b_Gi=${PROBE_ID},a=q,t=d,f=24,s=1,v=1;AAAA\x1b\\`;
  return `${wrap ? passthrough(query) : query}\x1b[c`;
}

function graphicsReply(buffer: string): boolean | null {
  const needle = `Gi=${PROBE_ID};`;
  const at = buffer.indexOf(needle);
  if (at < 0) return null;
  const rest = buffer.slice(at + needle.length);
  if (rest.length < 2) return null;
  return rest.startsWith("OK");
}

export function sixelReply(buffer: string): boolean | null {
  const idx = buffer.indexOf("\x1b[?");
  if (idx < 0) return null;
  const term = buffer.indexOf("c", idx);
  if (term < 0) return null;
  const segment = buffer.slice(idx, term + 1);
  const m = /\x1b\[\?([0-9;]*)c/.exec(segment);
  if (!m) return null;
  const params = m[1].split(";").map(Number);
  if (params.includes(4)) return true;
  return false;
}

function itermReply(_buffer: string): boolean | null {
  return null;
}

export function detectBackend(buffer: string): GraphicsBackend | null {
  if (graphicsReply(buffer) === true) return "kitty";
  if (sixelReply(buffer) === true) return "sixel";
  if (itermReply(buffer) === true) return "iterm";
  return null;
}

export function forcedBackend(env: NodeJS.ProcessEnv = process.env): GraphicsBackend | null {
  const raw = env[BACKEND_ENV]?.toLowerCase();
  if (raw === "kitty" || raw === "sixel" || raw === "iterm") return raw;
  return null;
}

/** Asks the terminal whether it can draw images. Only works on a real tty. */
export function probeGraphics(terminal: Terminal | null): Promise<GraphicsSupport> {
  const forced = forcedBackend();
  if (forced) return Promise.resolve("supported");

  const stdin = process.stdin;
  if (!stdin.isTTY || !process.stdout.isTTY || !stdin.setRawMode) {
    return Promise.resolve("unknown");
  }

  const wasRaw = stdin.isRaw;

  return new Promise<GraphicsSupport>((resolve) => {
    let buffer = "";
    let timer: NodeJS.Timeout | null = null;
    let done = false;

    const finish = (graphics: GraphicsSupport) => {
      if (done) return;
      done = true;
      if (timer) clearTimeout(timer);
      stdin.off("data", onData);
      try {
        if (!wasRaw) stdin.setRawMode?.(false);
      } catch { }
      if (!stdin.isPaused()) stdin.pause();
      resolve(graphics);
    };

    const onData = (chunk: Buffer) => {
      buffer += chunk.toString("binary");
      const backend = detectBackend(buffer);
      if (backend) return finish("supported");
      const kitty = graphicsReply(buffer);
      const sixel = sixelReply(buffer);
      if (kitty === false && sixel === false) return finish("unsupported");
      if (buffer.length > 4096) {
        const hasKittyReply = buffer.includes(`Gi=${PROBE_ID};`);
        if (hasKittyReply) return finish("unsupported");
        return finish("unknown");
      }
    };

    try {
      stdin.setRawMode(true);
    } catch {
      return finish("unknown");
    }
    stdin.resume();
    stdin.on("data", onData);

    timer = setTimeout(() => {
      const backend = detectBackend(buffer);
      if (backend) return finish("supported");
      const kitty = graphicsReply(buffer);
      const sixel = sixelReply(buffer);
      if (kitty === false || sixel === false) return finish("unsupported");
      return finish("unknown");
    }, PROBE_TIMEOUT_MS);

    try {
      fs.writeSync(1, probeSequence(terminal?.wrapper === "tmux"));
    } catch {
      finish("unknown");
    }
  });
}

export function probeBackend(terminal: Terminal | null): Promise<GraphicsBackend | null> {
  const forced = forcedBackend();
  if (forced) return Promise.resolve(forced);
  const stdin = process.stdin;
  if (!stdin.isTTY || !process.stdout.isTTY || !stdin.setRawMode) {
    return Promise.resolve(null);
  }
  const wasRaw = stdin.isRaw;
  return new Promise<GraphicsBackend | null>((resolve) => {
    let buffer = "";
    let timer: NodeJS.Timeout | null = null;
    let done = false;
    const finish = (backend: GraphicsBackend | null) => {
      if (done) return;
      done = true;
      if (timer) clearTimeout(timer);
      stdin.off("data", onData);
      try {
        if (!wasRaw) stdin.setRawMode?.(false);
      } catch {}
      if (!stdin.isPaused()) stdin.pause();
      resolve(backend);
    };
    const onData = (chunk: Buffer) => {
      buffer += chunk.toString("binary");
      const backend = detectBackend(buffer);
      if (backend) return finish(backend);
      if (buffer.length > 4096) return finish(null);
    };
    try {
      stdin.setRawMode(true);
    } catch {
      return finish(null);
    }
    stdin.resume();
    stdin.on("data", onData);
    timer = setTimeout(() => finish(detectBackend(buffer)), PROBE_TIMEOUT_MS);
    try {
      fs.writeSync(1, probeSequence(terminal?.wrapper === "tmux"));
    } catch {
      finish(null);
    }
  });
}

export function panePixels(): Promise<{ width: number; height: number } | null> {
  const stdin = process.stdin;
  if (!stdin.isTTY || !process.stdout.isTTY || !stdin.setRawMode) return Promise.resolve(null);

  const wasRaw = stdin.isRaw;
  return new Promise((resolve) => {
    let buffer = "";
    let timer: NodeJS.Timeout | null = null;
    let done = false;

    const finish = (value: { width: number; height: number } | null) => {
      if (done) return;
      done = true;
      if (timer) clearTimeout(timer);
      stdin.off("data", onData);
      try {
        if (!wasRaw) stdin.setRawMode?.(false);
      } catch { }
      if (!stdin.isPaused()) stdin.pause();
      resolve(value);
    };

    const onData = (chunk: Buffer) => {
      buffer += chunk.toString("binary");
      const reply = /\x1b\[4;(\d+);(\d+)t/.exec(buffer);
      if (reply) finish({ width: Number(reply[2]), height: Number(reply[1]) });
    };

    try {
      stdin.setRawMode(true);
    } catch {
      return finish(null);
    }
    stdin.resume();
    stdin.on("data", onData);
    timer = setTimeout(() => finish(null), PROBE_TIMEOUT_MS);

    try {
      fs.writeSync(1, "\x1b[14t");
    } catch {
      finish(null);
    }
  });
}

export function unsupportedGraphicsMessage(color = false): string {
  const sgr = (code: string, text: string) => (color ? `\x1b[${code}m${text}\x1b[0m` : text);
  return [
    "",
    `  ${sgr("1", "This terminal cannot show images, which terminal-browser needs.")}`,
    "",
    `  ${sgr("2", "We recommend Ghostty:")}`,
    `  ${sgr("4", "https://ghostty.org/download")}`,
    "",
    `  ${sgr("2", "Note: any terminal that supports the kitty graphics protocol is supported")}`,
    `  ${sgr("2", "On Termux, Sixel is supported via the patched Termux app (DA ;4;)")}`,
    `  ${sgr("2", "You can also set TERMINAL_BROWSER_BACKEND=sixel|iterm|kitty to force a backend")}`,
    `  ${sgr("2", "or TERMINAL_BROWSER_SKIP_GRAPHICS_CHECK=1 to bypass the check")}`,
    "",
  ].join("\n");
}
