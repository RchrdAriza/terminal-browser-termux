import fs from "node:fs";
import path from "node:path";

export function inTermux(env: NodeJS.ProcessEnv = process.env): boolean {
  return Boolean(env.TERMUX_VERSION) || (env.PREFIX ?? "").startsWith("/data/data/com.termux/");
}

// Android apps get no root-owned sandbox helper, no user namespaces, no /dev/shm and no GPU chromium can use.
export const TERMUX_CHROMIUM_FLAGS = ["--no-sandbox", "--disable-dev-shm-usage", "--disable-gpu"];

export function termuxEnv(env: NodeJS.ProcessEnv): NodeJS.ProcessEnv {
  const prefix = env.PREFIX ?? "/data/data/com.termux/files/usr";
  const fonts = path.join(prefix, "glibc", "etc", "fonts", "fonts.conf");
  return {
    ...env,
    TMPDIR: env.TMPDIR ?? path.join(prefix, "tmp"),
    FONTCONFIG_FILE: env.FONTCONFIG_FILE ?? (fs.existsSync(fonts) ? fonts : undefined),
  };
}
