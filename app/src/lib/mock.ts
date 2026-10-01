import type { Report, ScanEvent, ScanConfig } from "./types";

/* Realistic stand-in for a real scan on this machine, shaped exactly like
   the core Report model so the UI can be reviewed without the Rust shell. */

const day = 86_400_000;
const ago = (days: number) => new Date(Date.now() - days * day).toISOString();

export const MOCK_REPORT: Report = {
  groups: [
    { category: "low", count: 15, bytes: 30_389_334_224 },
    { category: "medium", count: 5, bytes: 11_272_233_984 },
    { category: "high", count: 5, bytes: 4_829_634_560 },
  ],
  warnings: [
    "The river ran murky — Spotlight recency data was incomplete for 41 items.",
  ],
  cost_cap: false,
  entries: [
    /* ---------------- LOW personal risk ---------------- */
    {
      candidate: {
        id: "c001",
        path: "/Users/you/Library/Caches/com.epicgames.launcher",
        kind: "folder",
        size_bytes: 3_655_209_344,
        kind_stats: { children: 214, files: 3_981, cache_like_ratio: 0.92, truncated: false },
        last_used_date: ago(412),
        last_used_from_spotlight: true,
        modified_date: ago(412),
        orphan_status: "orphaned",
        scope_category: "low",
        score: 93.2,
        score_components: { size: 25, age: 25, cache_loc: 15, orphan: 15, depth: 3, file_type: 9.2, child_count: 1 },
        band: "high",
        auto_high_basis: "cache_root",
      },
      tier: 1,
      tier_source: "argued_auto_high",
      summary:
        "Scored straight to Driftwood: it sits under ~/Library/Caches and its drift score landed in the top band. No AI reasoned about this folder — treat this tier as unverified, not cleared.",
      reasoning:
        "Auto-high heuristic (no AI reasoning was spent on this item):\n· Location: ~/Library/Caches — a cache/log root DriftWood treats as driftwood by definition.\n· Orphan status: orphaned — the owning app is no longer installed.\n· Drift score 93.2 of 100 landed in the top band (top 25% of this scan's scores): size 25.0, age 25.0, cache location 15.0, orphan 15.0, depth 3.0, file type 9.2, child count 1.0.\nThe tier is a heuristic verdict — it says \"this looks like driftwood by location and score\", not \"this is safe to clear\". Use \"Ask the river\" on the card for a real second opinion.",
      confidence: 1,
      privacy_tier_used: "minimal",
    },
    {
      candidate: {
        id: "c002",
        path: "/Users/you/Library/Application Support/Kagi",
        kind: "folder",
        size_bytes: 1_288_490_188,
        kind_stats: { children: 9, files: 1_204, cache_like_ratio: 0.61, truncated: false },
        last_used_date: ago(630),
        last_used_from_spotlight: true,
        modified_date: ago(630),
        orphan_status: "orphaned",
        scope_category: "low",
        score: 89.7,
        score_components: { size: 22, age: 25, cache_loc: 0, orphan: 15, depth: 4, file_type: 12.7, child_count: 3 },
        band: "high",
        auto_high_basis: "cache_root",
      },
      tier: 1,
      tier_source: "argued_auto_high",
      summary:
        "Scored straight to Driftwood: it sits under ~/Library/Caches and its drift score landed in the top band. No AI reasoned about this folder — treat this tier as unverified, not cleared.",
      reasoning:
        "Auto-high heuristic (no AI reasoning was spent on this item):\n· Location: ~/Library/Caches — a cache/log root DriftWood treats as driftwood by definition.\n· Orphan status: orphaned — the owning app is no longer installed.\n· Drift score 89.7 of 100 landed in the top band (top 25% of this scan's scores): size 22.0, age 25.0, cache location 0.0, orphan 15.0, depth 4.0, file type 12.7, child count 3.0.\nThe tier is a heuristic verdict — it says \"this looks like driftwood by location and score\", not \"this is safe to clear\". Use \"Ask the river\" on the card for a real second opinion.",
      confidence: 1,
      privacy_tier_used: "minimal",
    },
    {
      candidate: {
        id: "c003",
        path: "/Users/you/.npm/_cacache",
        kind: "folder",
        size_bytes: 1_142_593_536,
        kind_stats: { children: 61, files: 84_213, cache_like_ratio: 0.97, truncated: true },
        last_used_date: ago(96),
        last_used_from_spotlight: true,
        modified_date: ago(96),
        orphan_status: "active",
        scope_category: "low",
        score: 71.4,
        score_components: { size: 20, age: 14, cache_loc: 15, orphan: 0, depth: 2, file_type: 10, child_count: 4 },
        band: "middle",
      },
      tier: 1,
      tier_source: "llm",
      summary:
        "npm's package download cache. Purely a convenience cache — the next install simply re-downloads what it needs.",
      reasoning:
        "This is the standard content-addressable store for npm. It contains only fetched package tarballs; deleting it cannot break any project. npm will silently re-fetch packages on the next install, costing a little bandwidth. Size vs. benefit clearly favors clearing, though it will slowly rebuild itself.",
      confidence: 0.96,
      llm_model: "anthropic/claude-haiku-class",
      privacy_tier_used: "standard",
    },
    {
      candidate: {
        id: "c004",
        path: "/Users/you/Library/Caches/pip",
        kind: "folder",
        size_bytes: 398_458_880,
        kind_stats: { children: 12, files: 6_140, cache_like_ratio: 0.94, truncated: false },
        last_used_date: ago(58),
        last_used_from_spotlight: true,
        orphan_status: "active",
        scope_category: "low",
        score: 64.1,
        score_components: { size: 16, age: 10, cache_loc: 15, orphan: 0, depth: 2, file_type: 10, child_count: 2 },
        band: "middle",
      },
      tier: 1,
      tier_source: "llm",
      summary:
        "Python's wheel cache from pip installs. Rebuilt on demand; safe to clear whenever you like.",
      reasoning:
        "pip stores downloaded wheels here so repeat installs are instant. There is no configuration or state in this folder — only downloaded artifacts. Removal is harmless and reversible by definition of a cache.",
      confidence: 0.95,
      llm_model: "anthropic/claude-haiku-class",
      privacy_tier_used: "standard",
    },
    {
      candidate: {
        id: "c005",
        path: "/tmp/pip-unpack-8f3a2c",
        kind: "folder",
        size_bytes: 88_080_384,
        kind_stats: { children: 3, files: 47, cache_like_ratio: 0.88, truncated: false },
        modified_date: ago(171),
        last_used_from_spotlight: false,
        orphan_status: "unknown",
        scope_category: "low",
        score: 68.3,
        score_components: { size: 13, age: 18, cache_loc: 15, orphan: 0, depth: 5, file_type: 10, child_count: 4.3 },
        band: "middle",
      },
      tier: 1,
      tier_source: "llm",
      summary:
        "Leftover temp folder from an interrupted pip install five months ago. Temp files like this are meant to vanish.",
      reasoning:
        "The naming pattern (pip-unpack-XXXXXXXX) is pip's scratch space for unpacking wheels mid-install. An interrupted process left it behind. macOS clears /tmp periodically anyway; nothing references it.",
      confidence: 0.93,
      llm_model: "anthropic/claude-haiku-class",
      privacy_tier_used: "standard",
    },
    {
      candidate: {
        id: "c006",
        path: "/Users/you/Library/Logs/CoreSimulator",
        kind: "folder",
        size_bytes: 2_254_853_120,
        kind_stats: { children: 8, files: 12_942, cache_like_ratio: 0.71, truncated: false },
        last_used_date: ago(240),
        last_used_from_spotlight: true,
        orphan_status: "active",
        scope_category: "low",
        score: 74.8,
        score_components: { size: 22, age: 20, cache_loc: 15, orphan: 0, depth: 3, file_type: 8.8, child_count: 2 },
        band: "middle",
      },
      tier: 1,
      tier_source: "llm",
      summary:
        "Old simulator logs from iOS development. Logs, not data — regenerated every time Xcode runs a simulator.",
      reasoning:
        "These are diagnostic logs written by CoreSimulator runs, not simulator state or projects. The owning app (Xcode) is installed and manages the folder, so the auto-high heuristic correctly declined to fire — a live tool's log directory gets argued, not assumed. Deleting removes only historical logs; simulators recreate the directory on their next run.",
      confidence: 0.94,
      llm_model: "anthropic/claude-haiku-class",
      privacy_tier_used: "standard",
    },
    {
      candidate: {
        id: "c007",
        path: "/Users/you/Library/Caches/com.spotify.client",
        kind: "folder",
        size_bytes: 2_936_010_752,
        kind_stats: { children: 14, files: 28_411, cache_like_ratio: 0.9, truncated: false },
        last_used_date: ago(44),
        last_used_from_spotlight: true,
        orphan_status: "active",
        scope_category: "low",
        score: 66.9,
        score_components: { size: 22, age: 9, cache_loc: 15, orphan: 0, depth: 2, file_type: 10, child_count: 3 },
        band: "middle",
      },
      tier: 1,
      tier_source: "llm",
      summary:
        "Spotify's streamed-music cache at its default cap. Deleting only costs you offline playback until it refills.",
      reasoning:
        "Spotify keeps streamed audio here up to a storage cap and manages eviction itself. No playlists, credentials or settings live in this folder (those are in Preferences and Application Support). Clearing is safe; Spotify will re-stream songs you play next.",
      confidence: 0.94,
      llm_model: "anthropic/claude-haiku-class",
      privacy_tier_used: "standard",
    },
    {
      candidate: {
        id: "c008",
        path: "/Users/you/Library/Developer/Xcode/DerivedData",
        kind: "folder",
        size_bytes: 6_657_390_592,
        kind_stats: { children: 23, files: 214_889, cache_like_ratio: 0.83, truncated: true },
        last_used_date: ago(151),
        last_used_from_spotlight: true,
        orphan_status: "active",
        scope_category: "low",
        score: 78.2,
        score_components: { size: 25, age: 16, cache_loc: 15, orphan: 0, depth: 4, file_type: 8.2, child_count: 2 },
        band: "middle",
      },
      tier: 1,
      tier_source: "llm",
      summary:
        "Xcode build artifacts from projects last touched five months ago. Xcode rebuilds these on the next build — its own docs recommend clearing them.",
      reasoning:
        "DerivedData holds per-project build products, indexes, and logs. Xcode is installed and recreates everything here from the project sources on the next build, so nothing is personal or hard to reproduce. Live app, argued tier: the only cost of clearing is the first rebuild being slower.",
      confidence: 0.95,
      llm_model: "anthropic/claude-haiku-class",
      privacy_tier_used: "standard",
    },
    {
      candidate: {
        id: "c009",
        path: "/Users/you/Library/Application Support/Steam/SteamApps/htmlcache",
        kind: "folder",
        size_bytes: 512_449_536,
        kind_stats: { children: 4, files: 8_041, cache_like_ratio: 0.9, truncated: false },
        last_used_date: ago(67),
        last_used_from_spotlight: true,
        orphan_status: "active",
        scope_category: "low",
        score: 58.6,
        score_components: { size: 14, age: 11, cache_loc: 15, orphan: 0, depth: 5, file_type: 10, child_count: 1 },
        band: "middle",
      },
      tier: 3,
      tier_source: "llm",
      summary:
        "Steam's embedded-browser cache. Safe to clear, but Steam itself has been idle for two months — you may be about to play again.",
      reasoning:
        "This is render-view cache for the Steam client UI and store pages, not game data. It is regenerable, but since Steam was last used recently-ish and the cache speeds up the storefront, the practical cost of clearing is minor but nonzero. Clear if you need the space; ignore if not.",
      confidence: 0.82,
      llm_model: "anthropic/claude-haiku-class",
      privacy_tier_used: "standard",
    },
    {
      candidate: {
        id: "c010",
        path: "/Users/you/.gradle/caches",
        kind: "folder",
        size_bytes: 2_041_026_560,
        kind_stats: { children: 31, files: 96_722, cache_like_ratio: 0.76, truncated: true },
        last_used_date: ago(209),
        last_used_from_spotlight: true,
        orphan_status: "active",
        scope_category: "low",
        score: 72.5,
        score_components: { size: 22, age: 20, cache_loc: 0, orphan: 0, depth: 3, file_type: 10, child_count: 4.5 },
        band: "middle",
      },
      tier: 2,
      tier_source: "llm",
      summary:
        "Gradle's dependency cache from Android/JVM work seven months ago. Re-downloadable, but the next Gradle build will be slow while it refills.",
      reasoning:
        "Everything here is downloadable artifacts (jars, poms, transformed deps). Deletion is safe, but if you return to JVM projects the first build will re-fetch hundreds of megabytes and take noticeably longer. That trade-off — safe, but annoying to lose — puts it in the bottle tier.",
      confidence: 0.88,
      llm_model: "anthropic/claude-haiku-class",
      privacy_tier_used: "standard",
    },
    {
      candidate: {
        id: "c011",
        path: "/Users/you/go/pkg/mod",
        kind: "folder",
        size_bytes: 2_577_447_936,
        kind_stats: { children: 402, files: 41_008, cache_like_ratio: 0.51, truncated: true },
        last_used_date: ago(118),
        last_used_from_spotlight: true,
        orphan_status: "active",
        scope_category: "low",
        score: 70.9,
        score_components: { size: 23, age: 14, cache_loc: 0, orphan: 0, depth: 4, file_type: 6.1, child_count: 4.8 },
        band: "middle",
      },
      tier: 2,
      tier_source: "llm",
      summary:
        "Go module cache. Technically disposable, but Go rebuilds from it constantly during development — losing it means re-downloading the ecosystem.",
      reasoning:
        "The module cache holds verified source of every dependency version you've built against. `go clean -modcache` clears it safely, but any subsequent build re-downloads and re-verifies everything. If Go work has paused for four months, this is prime space; if it resumes, it's a tax.",
      confidence: 0.9,
      llm_model: "anthropic/claude-haiku-class",
      privacy_tier_used: "standard",
    },
    {
      candidate: {
        id: "c024",
        path: "/Users/you/.npm/_logs",
        kind: "folder",
        size_bytes: 46_891_520,
        kind_stats: { children: 2, files: 312, cache_like_ratio: 0.89, truncated: false },
        last_used_date: ago(96),
        last_used_from_spotlight: true,
        modified_date: ago(96),
        orphan_status: "active",
        scope_category: "low",
        score: 69.8,
        score_components: { size: 8, age: 14, cache_loc: 15, orphan: 0, depth: 2, file_type: 10, child_count: 1.8 },
        band: "middle",
      },
      tier: 1,
      tier_source: "llm_propagated",
      summary:
        "npm's debug logs, same parent folder as its download cache — one judgment covers the group.",
      reasoning:
        "This folder shares _cacache's parent, orphan status, and cache-like profile, so the river's verdict for the npm cache applies here too: pure scratch output, regenerated on every run. It was never judged separately.",
      confidence: 0.96,
      llm_model: "anthropic/claude-haiku-class",
      privacy_tier_used: "standard",
    },
    {
      candidate: {
        id: "c012",
        path: "/Users/you/Library/Caches/Homebrew/api-analytics",
        kind: "folder",
        size_bytes: 51_380_224,
        kind_stats: { children: 2, files: 118, cache_like_ratio: 0.95, truncated: false },
        last_used_from_spotlight: false,
        modified_date: ago(35),
        orphan_status: "active",
        scope_category: "low",
        score: 52.7,
        score_components: { size: 11, age: 7, cache_loc: 15, orphan: 0, depth: 3, file_type: 10, child_count: 1 },
        band: "middle",
      },
      tier: 1,
      tier_source: "fallback",
      summary:
        "Homebrew's API metadata cache. (Stage 2 reasoning was unavailable for this item — labeled by heuristics alone.)",
      reasoning: "",
      confidence: 0.5,
      privacy_tier_used: "minimal",
    },
    {
      // An Express-scan middle-band item: the river was never asked. The
      // provenance must read as by-design heuristic, not as a failure.
      candidate: {
        id: "c025",
        path: "/Users/you/Library/Containers/com.division.Paralogue/Data/cache",
        kind: "folder",
        size_bytes: 12_642_880,
        kind_stats: { children: 4, files: 61, cache_like_ratio: 0.7, truncated: false },
        last_used_date: ago(240),
        last_used_from_spotlight: false,
        modified_date: ago(240),
        orphan_status: "unknown",
        scope_category: "medium",
        score: 44.1,
        score_components: { size: 5.5, age: 16, cache_loc: 0, orphan: 0, depth: 3, file_type: 7, child_count: 0.6 },
        band: "middle",
      },
      tier: 3,
      tier_source: "heuristic",
      summary:
        "Heuristic estimate — this scan never asked the river about this item.",
      reasoning:
        "No AI reasoning was spent on this item: the scan mode does not cross the river (Express scan, or no OpenRouter key configured). The tier comes from the drift score (44.1/100) and the still-in-the-current rule — it is a heuristic guess, honestly labeled. \"Ask the river\" on this card gives a real second opinion, and Standard or Deep read mode argues every middle-band item.",
      confidence: 0.3,
      privacy_tier_used: "standard",
    },
    {
      candidate: {
        id: "c013",
        path: "/Users/you/Library/Caches/com.apple.HomeKit",
        kind: "folder",
        size_bytes: 6_204_450_000,
        kind_stats: { children: 3, files: 48, cache_like_ratio: 0.3, truncated: false },
        last_used_date: ago(380),
        last_used_from_spotlight: true,
        orphan_status: "active",
        scope_category: "low",
        score: 58.2,
        score_components: { size: 20, age: 20, cache_loc: 15, orphan: 0, depth: 2, file_type: 1.2, child_count: 0 },
        band: "middle",
      },
      tier: 3,
      tier_source: "system_floor",
      summary:
        "System-managed folder (com.apple.*). DriftWood won't call this safe to clear.",
      reasoning:
        "The folder's name matches the system-owner list (com.apple.*): the vendor owns the OS or the whole suite, so this folder may be load-bearing for things outside that one application. DriftWood declines to opine — a path-shape rule is not a safety argument, and it will not call this safe to clear without reasoning.\nThis is NOT a judgment that the data is precious or that reacquiring it would hurt: DriftWood simply refuses to guess here. A user rule can re-stamp it (your re-stamps beat the floor), and \"Ask the river\" on the card gives a second opinion.",
      confidence: 1,
      privacy_tier_used: "minimal",
    },
    {
      candidate: {
        id: "c013b",
        path: "/Users/you/Library/Application Support/MobileSync/Backup",
        kind: "folder",
        size_bytes: 685_792_256,
        kind_stats: { children: 4, files: 9_244, cache_like_ratio: 0.12, truncated: false },
        last_used_date: ago(402),
        last_used_from_spotlight: true,
        orphan_status: "active",
        scope_category: "low",
        score: 61.3,
        score_components: { size: 15, age: 20, cache_loc: 0, orphan: 0, depth: 4, file_type: 0.3, child_count: 0 },
        band: "middle",
      },
      tier: 4,
      tier_source: "never_flag",
      summary:
        "iPhone backup data. On DriftWood's never-flag list — personal, irreplaceable history.",
      reasoning: "",
      confidence: 1,
      privacy_tier_used: "minimal",
    },

    /* ---------------- MEDIUM personal risk ---------------- */
    {
      candidate: {
        id: "c014",
        path: "/Users/you/Downloads/HeroPress-4.2.dmg",
        kind: "file",
        size_bytes: 1_288_490_188,
        last_used_date: ago(151),
        last_used_from_spotlight: true,
        created_date: ago(151),
        orphan_status: "unknown",
        scope_category: "medium",
        score: 55.4,
        score_components: { size: 14, age: 16, cache_loc: 0, orphan: 0, depth: 0, file_type: 10, child_count: 0 },
        band: "middle",
      },
      tier: 2,
      tier_source: "rule",
      summary:
        "Disk image installer, mounted once in March and never touched since. Your own rule pins .dmg files in Downloads to this tier.",
      reasoning: "",
      confidence: 1,
      privacy_tier_used: "standard",
      rule_id: "dmg-in-downloads",
    },
    {
      candidate: {
        id: "c015",
        path: "/Users/you/Downloads/website-backup-2019.zip",
        kind: "file",
        size_bytes: 3_328_599_552,
        last_used_date: ago(1_140),
        last_used_from_spotlight: false,
        created_date: ago(1_140),
        orphan_status: "unknown",
        scope_category: "medium",
        score: 62.8,
        score_components: { size: 22, age: 25, cache_loc: 0, orphan: 0, depth: 0, file_type: 4.8, child_count: 0 },
        band: "middle",
      },
      tier: 3,
      tier_source: "llm",
      summary:
        "A three-year-old backup archive of a website. Unread for three years — but backups are exactly the thing you want the day you want them.",
      reasoning:
        "The name strongly suggests an intentional backup rather than debris, and archives of sites are often unreproducible (databases, old CMS versions). It has not been opened since 2023, which suggests it could go — but the downside of a wrong call here is permanent, so this stays in the current until you say otherwise.",
      confidence: 0.78,
      llm_model: "anthropic/claude-haiku-class",
      privacy_tier_used: "standard",
    },
    {
      candidate: {
        id: "c016",
        path: "/Users/you/Downloads/ubuntu-24.04-live-server.iso",
        kind: "file",
        size_bytes: 4_939_212_800,
        last_used_date: ago(88),
        last_used_from_spotlight: true,
        created_date: ago(95),
        orphan_status: "unknown",
        scope_category: "medium",
        score: 53.1,
        score_components: { size: 20, age: 9, cache_loc: 0, orphan: 0, depth: 0, file_type: 4.1, child_count: 0 },
        band: "middle",
      },
      tier: 2,
      tier_source: "llm",
      summary:
        "Linux installer image, three months old. Freely re-downloadable — a pure space-versus-convenience call.",
      reasoning:
        "ISOs are byte-identical to their upstream copies; nothing personal is at stake. But 4.9 GB of bandwidth and a slow mirror await anyone who needs it again. You wrote to it once and haven't mounted it since — likely burned onto a server already.",
      confidence: 0.91,
      llm_model: "anthropic/claude-haiku-class",
      privacy_tier_used: "standard",
    },
    {
      candidate: {
        id: "c017",
        path: "/Users/you/Library/Containers/com.stackideas.dev",
        kind: "folder",
        size_bytes: 932_177_920,
        kind_stats: { children: 6, files: 2_411, cache_like_ratio: 0.42, truncated: false },
        last_used_date: ago(275),
        last_used_from_spotlight: true,
        modified_date: ago(275),
        orphan_status: "orphaned",
        scope_category: "medium",
        score: 77.6,
        score_components: { size: 18, age: 23, cache_loc: 0, orphan: 15, depth: 3, file_type: 6.6, child_count: 2 },
        band: "high",
        auto_high_basis: "quantile_band",
      },
      tier: 1,
      tier_source: "argued_auto_high",
      summary:
        "Scored straight to Driftwood: its drift score landed in the top 25% of this scan's scores. No AI reasoned about this folder — treat this tier as unverified, not cleared.",
      reasoning:
        "Auto-high heuristic (no AI reasoning was spent on this item):\n· Location: no location rule fired — the top quantile band alone put this here.\n· Orphan status: orphaned — the owning app is no longer installed.\n· Drift score 77.6 of 100 landed in the top band (top 25% of this scan's scores): size 18.0, age 23.0, cache location 0.0, orphan 15.0, depth 3.0, file type 6.6, child count 2.0.\nThe tier is a heuristic verdict — it says \"this looks like driftwood by location and score\", not \"this is safe to clear\". Use \"Ask the river\" on the card for a real second opinion.",
      confidence: 1,
      privacy_tier_used: "minimal",
    },
    {
      candidate: {
        id: "c018",
        path: "/Users/you/Downloads/interview-takehome-final.mov",
        kind: "file",
        size_bytes: 734_003_200,
        last_used_date: ago(64),
        last_used_from_spotlight: true,
        created_date: ago(210),
        orphan_status: "unknown",
        scope_category: "medium",
        score: 44.9,
        score_components: { size: 14, age: 9, cache_loc: 0, orphan: 0, depth: 0, file_type: 0.9, child_count: 0 },
        band: "middle",
      },
      tier: 3,
      tier_source: "llm",
      summary:
        "A screen recording from a job application last winter. Likely reproducible, but it cost real effort to make.",
      reasoning:
        "Screen recordings of take-home work are re-recordable in principle, but the specific session (with your live commentary) is gone once deleted. Two months of disuse says the process is over; keep only if you might revisit the material.",
      confidence: 0.8,
      llm_model: "anthropic/claude-haiku-class",
      privacy_tier_used: "standard",
    },

    /* ---------------- HIGH personal risk ---------------- */
    {
      candidate: {
        id: "c019",
        path: "/Users/you/Documents/Minecraft/longshore-island",
        kind: "folder",
        size_bytes: 356_515_840,
        kind_stats: { children: 12, files: 4_802, cache_like_ratio: 0.03, truncated: false },
        last_used_date: ago(1_280),
        last_used_from_spotlight: true,
        modified_date: ago(1_280),
        orphan_status: "active",
        scope_category: "high",
        score: 49.2,
        score_components: { size: 13, age: 25, cache_loc: 0, orphan: 0, depth: 3, file_type: 0, child_count: 1.2 },
        band: "middle",
      },
      tier: 4,
      tier_source: "llm",
      summary:
        "A Minecraft world last played three and a half years ago. Deleting it is permanent — there is no re-download for a world you built.",
      reasoning:
        "This is user-created save data. Nothing about it is recoverable from the internet: the terrain, builds and inventory exist only here. Its age argues it no longer matters, but that is not knowable from metadata. The source tier exists precisely for this shape of thing.",
      confidence: 0.97,
      llm_model: "anthropic/claude-haiku-class",
      privacy_tier_used: "standard",
    },
    {
      candidate: {
        id: "c020",
        path: "/Users/you/Documents/taxes-2021",
        kind: "folder",
        size_bytes: 48_234_496,
        kind_stats: { children: 7, files: 61, cache_like_ratio: 0, truncated: false },
        last_used_date: ago(560),
        last_used_from_spotlight: true,
        orphan_status: "active",
        scope_category: "high",
        score: 31.5,
        score_components: { size: 5, age: 22, cache_loc: 0, orphan: 0, depth: 2, file_type: 0, child_count: 0.5 },
        band: "low",
      },
      tier: 4,
      tier_source: "auto_low",
      summary:
        "Tax records. Score landed it in the bottom band automatically — financial and personal, never a deletion candidate.",
      reasoning: "",
      confidence: 1,
      privacy_tier_used: "minimal",
    },
    {
      candidate: {
        id: "c021",
        path: "/Users/you/Pictures/2020-09-iceland-RAW",
        kind: "folder",
        size_bytes: 1_881_419_776,
        kind_stats: { children: 1, files: 318, cache_like_ratio: 0, truncated: false },
        last_used_date: ago(380),
        last_used_from_spotlight: true,
        orphan_status: "active",
        scope_category: "high",
        score: 38.9,
        score_components: { size: 18, age: 21, cache_loc: 0, orphan: 0, depth: 2, file_type: 0, child_count: 0 },
        band: "middle",
      },
      tier: 3,
      tier_source: "llm",
      summary:
        "RAW originals from an Iceland trip, unopened since the JPEGs were exported. The negatives of your negatives.",
      reasoning:
        "If final edited JPEGs exist elsewhere, these RAWs are the highest-quality source and cannot be re-created. Photographers often prune RAWs after final export, but that is a deliberate artistic decision, not a cleanup. It stays in the current; only you can say whether the edits are final.",
      confidence: 0.85,
      llm_model: "anthropic/claude-haiku-class",
      privacy_tier_used: "standard",
    },
    {
      candidate: {
        id: "c022",
        path: "/Users/you/Music/Bounce/stems-2022-11",
        kind: "folder",
        size_bytes: 2_212_862_976,
        kind_stats: { children: 9, files: 214, cache_like_ratio: 0, truncated: false },
        last_used_date: ago(640),
        last_used_from_spotlight: true,
        orphan_status: "active",
        scope_category: "high",
        score: 42.6,
        score_components: { size: 21, age: 25, cache_loc: 0, orphan: 0, depth: 3, file_type: 0, child_count: 0.8 },
        band: "middle",
      },
      tier: 3,
      tier_source: "llm",
      summary:
        "Rendered stems from a Logic session, ten months quiet. The project file could regenerate them — if it still exists.",
      reasoning:
        "Bounces are re-renderable from the parent .logicx project, so in principle disposable. But stems often include manual edits and the parent project's whereabouts is uncertain from here. Annoying-to-impossible to reconstruct: firmly in the current.",
      confidence: 0.76,
      llm_model: "anthropic/claude-haiku-class",
      privacy_tier_used: "standard",
    },
    {
      candidate: {
        id: "c023",
        path: "/Users/you/Documents/thesis-drafts-2017",
        kind: "folder",
        size_bytes: 123_433_776,
        kind_stats: { children: 5, files: 89, cache_like_ratio: 0.02, truncated: false },
        last_used_date: ago(1_620),
        last_used_from_spotlight: false,
        orphan_status: "active",
        scope_category: "high",
        score: 27.4,
        score_components: { size: 9, age: 25, cache_loc: 0, orphan: 0, depth: 2, file_type: 0, child_count: 0.4 },
        band: "low",
      },
      tier: 4,
      tier_source: "auto_low",
      summary:
        "Old writing drafts, untouched in years. Low drift score — but writing is personal history, and lands in Source regardless.",
      reasoning: "",
      confidence: 1,
      privacy_tier_used: "minimal",
    },
  ],
};

/** Notices/warnings stream for the disclosure feed during a mock scan. */
export const MOCK_NOTICES: Record<string, string[]> = {
  enumerating: [
    "mdfind -onlyin ~/Library/Caches …",
    "mdfind -onlyin ~/Downloads …",
    "spotlight blind spots detected — falling back to walkdir for /tmp",
  ],
  wading: [
    "mdls batch: kMDItemLastUsedDate for 512 items",
    "wading past 3 unreadable directories (permission denied)",
  ],
  filtering: [
    "still-in-the-current rule dropped 96 recent items",
    "orphan scan: 412 installed apps indexed, 23 vendors normalized",
  ],
  scoring: ["weights: size 25 · age 25 · cache 15 · orphan 15 · depth 5 · type 10 · children 5"],
  reasoning: [
    "traveling to the river — 31 candidates in 3 batches",
    "14 representative items stand in for 31 — their siblings share each verdict",
    "batch 1/3 — 10 representatives · zdr: true",
  ],
  assembling: ["grouping 23 findings by category"],
};

export function mockScanEvents(cfg: ScanConfig, fail: boolean): Array<[number, ScanEvent]> {
  const runsLlm = cfg.mode !== "express";
  const scale = cfg.scopes.length === 0 ? 0.2 : 0.5 + 0.35 * cfg.scopes.length;
  const script: Array<[number, ScanEvent]> = [
    [0, { type: "phase", phase: "enumerating" }],
    [400, { type: "files_searched", total: 4_120 }],
    [900, { type: "files_searched", total: 41_880 }],
    [1_200, { type: "notice", message: MOCK_NOTICES.enumerating[0] }],
    [1_700, { type: "files_searched", total: 214_400 }],
    [2_000, { type: "bytes_searched", total: 1_214_000_000 }],
    [2_400, { type: "notice", message: MOCK_NOTICES.enumerating[1] }],
    [2_900, { type: "files_searched", total: 668_000 }],
    [3_300, { type: "warn", message: MOCK_NOTICES.enumerating[2] }],
    [3_700, { type: "phase", phase: "wading" }],
    [4_000, { type: "notice", message: MOCK_NOTICES.wading[0] }],
    [4_600, { type: "files_searched", total: 940_000 }],
    [5_000, { type: "bytes_searched", total: 9_880_000_000 }],
    [5_300, { type: "notice", message: MOCK_NOTICES.wading[1] }],
    [5_900, { type: "candidates_found", total: 74 }],
    [6_400, { type: "phase", phase: "filtering" }],
    [6_800, { type: "notice", message: MOCK_NOTICES.filtering[0] }],
    [7_400, { type: "candidates_found", total: 118 }],
    [7_900, { type: "notice", message: MOCK_NOTICES.filtering[1] }],
    [
      8_100,
      {
        type: "notice",
        message:
          "3 system/vendor folders (9.1 GB) held at Current — DriftWood won't call them safe to clear",
      },
    ],
    // Conservative recoverable: only what is already definitely disposable
    // (auto-high, not floored/pinned). Corrections downward are possible
    // and honest — the store takes the latest, never the max.
    [8_400, { type: "recoverable_bytes", total: 6_410_000_000 }],
    [8_900, { type: "phase", phase: "scoring" }],
    [9_300, { type: "notice", message: MOCK_NOTICES.scoring[0] }],
    [9_900, { type: "candidates_found", total: 214 }],
    // The engine no longer restates an everything-sum mid-scan: the next
    // recoverable emission is the exact tiers-1–2 total after assembly.
    [10_800, { type: "phase", phase: "reasoning" }],
  ];
  if (runsLlm) {
    script.push(
      [11_200, { type: "notice", message: MOCK_NOTICES.reasoning[0] }],
      // Live Stage 2: batch signals + progress with a knowable denominator.
      [11_400, { type: "batch_started", index: 1, total_batches: 3 }],
      [12_100, { type: "batch_started", index: 2, total_batches: 3 }],
      [
        12_300,
        {
          type: "reasoning_progress",
          judged: 12,
          total: 31,
          cost_usd: 0.0041,
          prompt_tokens: 3_120,
          completion_tokens: 640,
        },
      ],
      [12_600, { type: "batch_finished", index: 1, total_batches: 3 }],
      [12_900, { type: "batch_started", index: 3, total_batches: 3 }],
      [
        13_400,
        {
          type: "reasoning_progress",
          judged: 24,
          total: 31,
          cost_usd: 0.0077,
          prompt_tokens: 6_240,
          completion_tokens: 1_280,
        },
      ],
      [13_900, { type: "batch_finished", index: 2, total_batches: 3 }],
      [
        14_400,
        {
          type: "reasoning_progress",
          judged: 31,
          total: 31,
          cost_usd: 0.0104,
          prompt_tokens: 9_360,
          completion_tokens: 1_920,
        },
      ],
      [14_700, { type: "batch_finished", index: 3, total_batches: 3 }],
    );
  } else {
    script.push([
      11_300,
      { type: "notice", message: "stage 2 skipped — heuristics only" },
    ]);
  }
  script.push(
    [15_400, { type: "phase", phase: "assembling" }],
    [15_800, { type: "notice", message: MOCK_NOTICES.assembling[0] }],
    // Exact post-assembly figure (tiers 1–2 only). It may sit below the
    // conservative mid-scan value — corrections downward are information.
    [16_100, { type: "recoverable_bytes", total: 5_820_000_000 }],
  );
  if (fail) {
    script.push([16_200, { type: "error", message: "Stage 2 lost mid-crossing: connection to the river dropped (network unreachable)." }]);
  }
  return script.map(([t, e]) => [Math.round(t * scale), e]);
}
