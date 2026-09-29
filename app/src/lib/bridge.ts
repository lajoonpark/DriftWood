import type { Report, RevealSummary, ScanConfig, ScanEvent, Tier } from "./types";
import { MOCK_REPORT, mockScanEvents } from "./mock";

/**
 * The only seam between the frontend and the world.
 *
 * Inside the Tauri shell these map to Rust commands (see app/src-tauri when
 * the scripting layer lands):
 *   check_full_disk_access() -> "granted" | "denied" | "unknown"
 *   open_system_settings()
 *   start_scan(config)            — emits `scan-event` windows
 *   cancel_scan()
 *   last_report() -> Report | null
 *   correct_tier(id, tier, note?)
 *   reveal_path(path)             — `open -R <path>`
 *   reveal_paths(paths) -> RevealSummary
 *                                 — ONE `open -R`, one window per folder,
 *                                   capped; the summary says what opened
 * In the browser (and until the shell is wired) a mock bridge answers, so
 * every screen is fully reviewable in `npm run dev`.
 */

export type FdaStatus = "granted" | "denied" | "unknown";

export interface Bridge {
  readonly name: string;
  checkFullDiskAccess(): Promise<FdaStatus>;
  openSystemSettings(): void;
  startScan(cfg: ScanConfig, onEvent: (e: ScanEvent) => void): Promise<Report>;
  cancelScan(): void;
  getLastReport(): Promise<Report | null>;
  correctTier(id: string, tier: Tier): Promise<void>;
  reveal(path: string): void;
  /** Hand a selection off to Finder (one window per folder, capped at a
   *  handful of windows — never a flood). Resolves with what actually
   *  opened so the UI can tell the user the truth. */
  revealAll(paths: string[]): Promise<RevealSummary>;
}

const w = () => window as unknown as {
  __TAURI_INTERNALS__?: { invoke: (cmd: string, args?: unknown) => Promise<unknown> };
};

function tauriInvoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  const internals = w().__TAURI_INTERNALS__;
  if (!internals) return Promise.reject(new Error("not inside the Tauri shell"));
  return internals.invoke(cmd, args) as Promise<T>;
}

class TauriBridge implements Bridge {
  readonly name = "tauri";
  async checkFullDiskAccess(): Promise<FdaStatus> {
    return tauriInvoke<FdaStatus>("check_full_disk_access");
  }
  openSystemSettings(): void {
    void tauriInvoke("open_system_settings");
  }
  async startScan(cfg: ScanConfig, onEvent: (e: ScanEvent) => void): Promise<Report> {
    const { listen } = await import("./tauri-events");
    const off = await listen<ScanEvent>("scan-event", onEvent);
    try {
      return await tauriInvoke<Report>("start_scan", { config: cfg });
    } finally {
      off?.();
    }
  }
  cancelScan(): void {
    void tauriInvoke("cancel_scan");
  }
  async getLastReport(): Promise<Report | null> {
    return tauriInvoke<Report | null>("last_report");
  }
  async correctTier(id: string, tier: Tier): Promise<void> {
    await tauriInvoke("correct_tier", { id, tier });
  }
  reveal(path: string): void {
    void tauriInvoke("reveal_path", { path });
  }
  revealAll(paths: string[]): Promise<RevealSummary> {
    return tauriInvoke<RevealSummary>("reveal_paths", { paths });
  }
}

class MockBridge implements Bridge {
  readonly name = "mock";
  private cancelled = false;
  private failScan = false;

  constructor() {
    if (typeof window !== "undefined") {
      const q = new URLSearchParams(window.location.search);
      this.failScan = q.has("snag");
    }
  }

  async checkFullDiskAccess(): Promise<FdaStatus> {
    await sleep(1_100);
    return "denied";
  }
  openSystemSettings(): void {
    /* mock — would deep-link to x-apple.systempreferences */
  }
  async startScan(cfg: ScanConfig, onEvent: (e: ScanEvent) => void): Promise<Report> {
    this.cancelled = false;
    const script = mockScanEvents(cfg, this.failScan);
    let elapsed = 0;
    for (const [t, event] of script) {
      const wait = t - elapsed;
      if (wait > 0) await sleep(wait);
      elapsed = t;
      if (this.cancelled) throw new Error("cancelled");
      onEvent(event);
      if (event.type === "error") throw new Error(event.message);
    }
    if (this.cancelled) throw new Error("cancelled");
    return MOCK_REPORT;
  }
  cancelScan(): void {
    this.cancelled = true;
  }
  async getLastReport(): Promise<Report | null> {
    return MOCK_REPORT;
  }
  async correctTier(): Promise<void> {
    await sleep(500);
  }
  reveal(): void {
    /* mock — would run `open -R <path>` */
  }
  async revealAll(paths: string[]): Promise<RevealSummary> {
    await sleep(700);
    const groups = new Map<string, number>();
    for (const p of paths) {
      const parent = p.split("/").slice(0, -1).join("/");
      groups.set(parent, (groups.get(parent) ?? 0) + 1);
    }
    const sizes = [...groups.values()].sort((a, b) => b - a);
    const windows = Math.min(sizes.length, 3);
    return {
      windows,
      items: sizes.slice(0, windows).reduce((n, s) => n + s, 0),
      skipped_groups: Math.max(0, sizes.length - windows),
      skipped_items: sizes.slice(windows).reduce((n, s) => n + s, 0),
    };
  }
}

function sleep(ms: number) {
  return new Promise((r) => setTimeout(r, ms));
}

export const bridge: Bridge =
  typeof window !== "undefined" && "__TAURI_INTERNALS__" in window
    ? new TauriBridge()
    : new MockBridge();
