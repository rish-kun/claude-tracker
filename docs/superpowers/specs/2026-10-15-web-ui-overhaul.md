# cctrack web UI overhaul: the work ledger

**Date:** 2026-10-15
**Status:** Design spec, ready for implementation
**Scope:** `apps/web` only (styles, components, pages, one new route). Same routes, same data, plus the new `Session.cwd`, `Session.title` and `GET /sessions/{sessionId}`.

The current UI reads as a generic SaaS kit: four identical stat cards led by a misleading "116.7M tokens", then charts and tables. The owner's requirement is "see **why** tokens were used". This redesign makes the session (a piece of work with a name) the main unit of the app, and every number links to the sessions behind it.

---

## 1. Principles and the one bold element

1. **Words for work, numbers for amounts.** What people did (session titles, summaries, the summary sentence) is set in a serif text face. Numbers and interface use a sans with tabular figures. Literal machine strings (paths, branches, session ids, commands) use the system monospace, and nothing else does.
2. **Every number leads to work.** Any total links to, or expands into, the sessions that make it up. Session rows link to the session detail page, which explains the cost drivers in sentences.
3. **Colour means constraint.** The plan-limit windows are what actually stop people, so only limit levels get warm colour (amber, coral). API-equivalent cost is a comparison figure, not money charged, so it is drawn in neutral ink and never in a warning or brand colour. The accent colour marks interaction state only (focus, selection, links).
4. **Tokens are split, never summed for a headline.** We say "read" (everything sent to Claude, mostly cache) and "written" (what Claude produced). A total token count never appears on its own.
5. **Quiet structure.** Content sits on the page background separated by space and hairlines. Raised panels are reserved for the limit strip, popovers and form areas. No shadows on content, no gradients, no ALL-CAPS labels, no middle-dot meta strings.

**The bold element: the ledger.** Sessions are presented as a day-grouped logbook of work, not a table at the bottom of the page. Each entry leads with its title in the serif at reading size, the summary beneath, a line of facts with small glyphs (folder, branch, device, account), and on the right the API-equivalent cost with a neutral weight bar so expensive work stands out at a glance. The ledger is the centre of My usage, the drill-down of By login, and the entry point to Session detail. Everything else stays disciplined around it.

Supporting detail with character, kept small: **segmented limit meters**, 20 cells of 5% each, echoing the block bars of Claude Code's own `/usage` screen.

### Review against the brief (what changed from the first draft)

- First draft used a marigold accent for cost bars. Changed: cost is not a warning and not money, so cost bars are neutral ink; warm colour is reserved for plan limits.
- First draft kept a row of four metric tiles. Replaced by one generated sentence in the serif plus a single-row token mix, because tiles were the exact thing the owner rejected.
- First draft put a monospace face on all small labels. Restricted mono to literal machine strings.
- First draft used a donut for "share of account". Replaced with labelled horizontal share bars (donuts make small shares unreadable and are template defaults).
- Session meta was joined with middle dots. Replaced with glyph-led facts separated by space.

---

## 2. Tokens

All tokens live in `apps/web/src/styles.css` as custom properties on `:root`, with dark values under `@media (prefers-color-scheme: dark)`. Add `<meta name="color-scheme" content="light dark">` to `index.html` and `color-scheme: light dark` on `:root`.

### 2.1 Colour

All text pairs below were checked for WCAG AA (ratios in brackets are measured against `--bg` unless noted).

| Token | Light | Dark | Use |
|---|---|---|---|
| `--bg` | `#F4F6FA` | `#141B2B` | page plane, chart surface |
| `--surface` | `#FFFFFF` | `#1B2337` | raised panels: limit strip, popovers, inputs, tooltips |
| `--surface-2` | `#EBEFF5` | `#232D44` | row hover, skeletons, avatar, selected-segment base |
| `--line` | `#D8DEE8` | `#2E3953` | hairlines, table rules, panel borders |
| `--line-strong` | `#7B879C` | `#6A789A` | input and button borders (≥3:1 on bg and surface) |
| `--grid` | `#E1E6EE` | `#253049` | chart gridlines |
| `--text` | `#172033` (15.0) | `#E8ECF4` (14.5) | primary text, figures |
| `--text-2` | `#3F4A60` (8.2) | `#B4BDCF` (9.1) | summaries, secondary text |
| `--text-3` | `#5E6A80` (5.0) | `#8C97AD` (5.9) | meta, axis ticks, captions (≥4.6 on surface-2 too) |
| `--accent` | `#0B7266` (5.4) | `#7FDBCA` (10.6) | focus ring, links, active nav underline, selected segment |
| `--on-accent` | `#FFFFFF` | `#0E2A27` | text on a solid accent fill |
| `--accent-wash` | `color-mix(in srgb, var(--accent) 14%, transparent)` | same | selected segment background |
| `--weight` | `#78849A` | `#6C7A98` | cost weight bars (≥3:1 on bg, surface, surface-2) |
| `--meter-empty` | `#DCE2EC` | `#2C3753` | unfilled meter cells |
| `--meter-ok` | `#3A4660` | `#C9D2E3` | filled cells below 70% |
| `--meter-high` | `#B86E00` | `#F2B441` | filled cells 70–89% (graphic ≥3:1 on surface) |
| `--meter-crit` | `#C8372C` | `#F27765` | filled cells ≥90% |
| `--warn-text` | `#8F5500` | `#F2B441` | "Getting full", "Plan not set" text |
| `--crit-text` | `#B3302A` | `#F27765` | "Nearly full", errors, destructive buttons |
| `--warn-wash` | `#FBF1DE` | `#2E2A22` | plan-not-set chip and notice background |
| `--crit-wash` | `#FBE7E4` | `#33222A` | error notice background |

Token-kind ramp (ordinal, one neutral hue, brightest = what Claude wrote, dimmest = bulk cache reads). Always shown with text labels.

| Token | Light | Dark |
|---|---|---|
| `--kind-written` | `#172033` | `#E8ECF4` |
| `--kind-input` | `#3F4A60` | `#B4BDCF` |
| `--kind-cachewrite` | `#6B778C` | `#8C97AD` |
| `--kind-cacheread` | `#A9B3C4` | `#5A6785` |

Chart series (categorical, fixed order, never cycled; validated with the dataviz validator on these surfaces: adjacent CVD ΔE ≥ 8.4, normal-vision ΔE ≥ 19.3 in both modes; light mode has sub-3:1 slots, so every chart ships a legend, a tooltip and a table view):

| Token | Light | Dark |
|---|---|---|
| `--series-1` | `#2A78D6` | `#3987E5` |
| `--series-2` | `#EB6834` | `#D95926` |
| `--series-3` | `#1BAF7A` | `#199E70` |
| `--series-4` | `#EDA100` | `#C98500` |
| `--series-5` | `#E87BA4` | `#D55181` |
| `--series-6` | `#6250D6` | `#9085E9` |
| `--series-other` | `#A9B3C4` | `#5A6785` |

More than 5 entities in one chart: the top 5 by range total keep slots 1–5, the rest fold into "Other" (`--series-other`). Slot 6 is only used by fixed-entity charts (none today; reserved).

### 2.2 Type

`index.html` gets:

```html
<link rel="preconnect" href="https://fonts.googleapis.com">
<link rel="preconnect" href="https://fonts.gstatic.com" crossorigin>
<link rel="stylesheet" href="https://fonts.googleapis.com/css2?family=Newsreader:ital,opsz,wght@0,6..72,400..600;1,6..72,400&family=Public+Sans:wght@400..700&display=swap">
```

```css
--font-serif: "Newsreader", "Iowan Old Style", Charter, Georgia, serif;
--font-sans: "Public Sans", system-ui, -apple-system, "Segoe UI", sans-serif;
--font-mono: ui-monospace, "SF Mono", Menlo, Consolas, monospace;
```

`body` uses `--font-sans`. Every element showing figures (`.fig`, tables, axes, meters) sets `font-variant-numeric: tabular-nums lining-nums`. Newsreader uses `font-optical-sizing: auto`.

| Role | Family | Size / line-height | Weight | Notes |
|---|---|---|---|---|
| `--t-display` | serif | 34/39 (≥860px), 27/32 below | 500 | session detail title, `letter-spacing: -0.01em` |
| `--t-sentence` | serif | 24/32 (≥860px), 20/28 below | 400 | My usage summary sentence; figures inside at 600 |
| `--t-h1` | sans | 22/28 | 600 | page titles |
| `--t-h2` | sans | 16/22 | 600 | section headings |
| `--t-entry` | serif | 17/23 | 500 | ledger titles |
| `--t-prose` | serif | 15/22 | 400 | summaries (colour `--text-2`) |
| `--t-body` | sans | 15/22 | 400 | UI text, table cells |
| `--t-meta` | sans | 13/19 | 400 | facts, captions, secondary cells |
| `--t-micro` | sans | 12/16 | 400/500 | axis ticks, footnotes, chips; never smaller |
| `--t-fig-lg` | sans | 28/32 | 600 | meter percentages, detail headline figures |
| `--t-fig` | sans | 17/22 | 600 | row cost |
| `--t-mono` | mono | 12.5/18 | 400 | paths, branches, ids, commands |

Sentence case everywhere. No letter-spaced caps. Measure: prose blocks max `68ch`.

### 2.3 Spacing, radii, borders, elevation, motion

```css
--s-1: 4px; --s-2: 8px; --s-3: 12px; --s-4: 16px; --s-5: 24px; --s-6: 32px; --s-7: 48px; --s-8: 64px;
--r-cell: 1.5px;  /* meter cells, bars, swatches */
--r-ctl: 6px;     /* inputs, buttons, chips, segmented control, row focus */
--r-panel: 10px;  /* limit strip, popovers, notices */
--hair: 1px solid var(--line);
--shadow-pop: 0 10px 28px rgb(20 30 60 / 0.14);           /* light */
--shadow-pop: 0 10px 28px rgb(3 6 16 / 0.55), 0 0 0 1px var(--line); /* dark */
--dur-1: 120ms; --dur-2: 200ms; --dur-3: 320ms;
--ease: cubic-bezier(0.2, 0, 0, 1);
--header-h: 56px;
--content-max: 1240px;
--gutter: 24px; /* 16px below 560px */
```

Breakpoints (literal in media queries): `560px` (phone), `860px` (tablet), `1100px` (two-column).

Only popovers (user menu, chart tooltip) carry `--shadow-pop`. Nothing else has a shadow.

Motion:
- One orchestrated moment: on first data render of a limit meter, filled cells appear left to right, 12ms stagger per cell, `opacity 0→1` over `--dur-2`. Not repeated on refetch.
- Responsive motion only: row hover background (`--dur-1`), disclosure expand (`--dur-2`, height via `grid-template-rows: 0fr→1fr`), menu open (`opacity` + 4px translate, `--dur-1`).
- Recharts: `isAnimationActive={false}` on every series.
- `@media (prefers-reduced-motion: reduce)`: all transitions and the meter stagger set to 0ms; skeleton pulse disabled.

Focus: `:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; border-radius: var(--r-ctl); }`. Ledger rows use `outline-offset: -2px` (inset) so the ring isn't clipped.

### 2.4 Glyphs (inline SVG, 16×16 viewBox, `stroke="currentColor"`, `stroke-width="1.5"`, `fill="none"`, round caps/joins, rendered at 14px, `aria-hidden="true"`)

| Name | Path data |
|---|---|
| folder | `M2 4.5h4l1.5 1.5H14v6.5H2z` |
| branch | circles `cx4 cy3.5 r1.5`, `cx4 cy12.5 r1.5`, `cx12 cy5 r1.5`; `M4 5v6M12 6.5c0 3-8 2-8 4.5` |
| device | `M3.5 3.5h9v7h-9zM1.5 13h13` |
| account (card) | `M2 4.5h12v8H2zM2 7.5h12` |
| person | circle `cx8 cy5.5 r2.5`; `M3 13.5c.8-2.6 2.8-4 5-4s4.2 1.4 5 4` |
| copy | `M5.5 5.5h7v7h-7zM3.5 10.5v-7h7` |
| check | `M3.5 8.5l3 3 6-7` |
| chevron-right | `M6 3.5 10.5 8 6 12.5` |
| chevron-down | `M3.5 6 8 10.5 12.5 6` |
| alert (filled, for limit levels) | `M8 1.8 15 14H1z` fill `currentColor`, stroke none; with `M8 6v4M8 11.6v.4` stroke `var(--surface)` |

---

## 3. App shell

### 3.1 Desktop (≥860px)

```
┌──────────────────────────────────────────────────────────────────────────────┐
│ cctrack    My usage   By login   By account   Settings ●              (R) Rishit ▾ │  56px, sticky
└──────────────────────────────────────────────────────────────────────────────┘
    My usage                                     [ Today | 7 days | 30 days | 90 days ]
                                                                   1 Oct – 15 Oct
    ───────────────────────────────────────────────────────────────────────────
    (page content, max-width 1240, gutters 24)
```

- **Header**: sticky, `background: var(--bg)`, bottom `--hair`, height `--header-h`. Wordmark "cctrack" in sans 600 18px, `--text`, links to `/`.
- **Nav**: links in sans 500 15px `--text-3`; hover `--text`; active (`data-status='active'`) `--text` with a 2px `--accent` bar flush with the header's bottom edge. Admin links only when `me.is_admin`. If any account has plan `unknown` (from `useAccounts`, admin only), Settings shows a 6px `--meter-high` dot plus visually hidden text "(an account needs a plan)".
- **User menu**: a button with a 28px circle (`--surface-2`, sans 600 13px initial of `me.name || me.email`) and the first name (hidden below 560px), chevron-down. Opens a popover (`--surface`, `--r-panel`, `--shadow-pop`, 240px): name (body 600), email (meta `--text-3`), a hairline, "Sign out" button. Escape and outside click close; focus returns to the trigger. Sign-out pending: "Signing out…". Failure: inline `--crit-text` "Couldn't sign out: {message}" inside the menu.
- **Range control lives in the page header, not the global header**, so it sits next to the content it changes. It is rendered only on My usage, By login and By account. Under it, a meta line shows the resolved range: `from` to `to − 1 min` as "1 Oct – 15 Oct" (`day: 'numeric', month: 'short'`), "Today" range shows "Since midnight".

### 3.2 Phone (<560px) and tablet

```
┌──────────────────────────────┐
│ cctrack                 (R) ▾ │
├──────────────────────────────┤
│ My usage  By login  By acc… → │  horizontally scrollable tab row, 44px
└──────────────────────────────┘
  My usage
  [Today|7 days|30 days|90 days]   full width, equal segments
  1 Oct – 15 Oct
```

- 560–859px: nav stays in the header row if it fits; otherwise moves to the second row as above. Implement with the second row always present below 860px.
- Tab row: `overflow-x: auto`, `scroll-snap-type: x proximity`, active tab scrolled into view on route change, touch targets ≥44px tall.
- Page header stacks: h1, then range control full width.

### 3.3 Page header component

`<PageHeader title="My usage" range />` renders h1 left and (when `range`) the RangeControl right, bottom `--hair`, `margin-bottom: var(--s-6)`.

**RangeControl**: `role="radiogroup"` `aria-label="Date range"`, four `role="radio"` buttons with `aria-checked`, roving tabindex, Left/Right arrows move and select. Segments: sans 500 14px, height 32px, padding 0 12px, container border 1px `--line-strong`, radius `--r-ctl`. Selected: `--accent-wash` background, `--text`, inset `box-shadow: inset 0 0 0 1px var(--accent)`. Re-clicking the selected preset keeps the existing behaviour (refetch).

### 3.4 Global states

- Initial `useMe` pending: centred wordmark plus meta "Loading your account…".
- `useMe` error (non-401): Notice (error) "Couldn't load your account: {message}." with a "Try again" button.
- Admin pages for non-admins: Notice (info) "This page is for admins. Ask a team admin if you need access." with a link "Go to my usage".

---

## 4. Pages

### 4.1 My usage (`/`)

**Purpose:** how much of my plan windows are used right now, what my usage amounts to over the range, and which sessions it went into.

```
≥1100px
  My usage                                            [Today|7 days|30 days|90 days]
  ─────────────────────────────────────────────────────────────────────────────────
  ┌ Plan windows ───────────────────────────────────────────────────────────────┐
  │ DVM admin   Max 20x                                    Reported 12 min ago   │
  │ 5-hour  ▮▮▮▮▮▮▮▮▮▮▮▮▯▯▯▯▯▯▯▯  62%   Resets in 2 h 14 min                    │
  │ 7-day   ▮▮▮▮▮▮▮▮▮▮▮▮▮▮▮▯▯▯▯▯  74%   Getting full. Resets Mon 20 Oct, 09:00  │
  └─────────────────────────────────────────────────────────────────────────────┘

  In the last 30 days, Claude read 114.2M tokens and wrote 1.4M for you
  across 2 accounts. At pay-as-you-go API prices that would be $40.17.
  API-equivalent only. The team's Max plans are flat-rate, so nobody pays this.

  Written 1.4M 1%   New input 212k <1%   Cache writes 3.1M 3%   Cache reads 109.5M 96%
  ▮▯▯▬▬▬▬▬▬▬▬▬▬▬▬▬▬▬▬▬▬▬▬▬▬▬▬▬▬▬▬▬▬▬▬▬▬▬▬▬▬▬▬▬▬▬▬▬▬▬▬▬▬▬▬▬▬▬▬▬▬  (6px token mix bar)

  API-equivalent cost per day, by model
  [legend]  ▇ ▇ ▅ ▇ ▂ ...  (220px)

  Latest sessions                                   │ By model
  All time, newest first                            │ ...
  Today ──────────────────────────────────────      │ By account
   14:05  BITS Pilani SWD website IIS migration  $32.04│ ...
          …                                        │
  [Newer]  21–40 of 132  [Older]                    │
```

Grid at ≥1100px: limit strip, sentence, token mix and chart span full width; below, `grid-template-columns: minmax(0, 1fr) 320px; gap: var(--s-7)` with the ledger left and the rail right. Below 1100px everything is one column in this order: limit strip, sentence, token mix, ledger, chart, by model, by account (the ledger moves above the chart on narrow screens because it is the main job).

**Data:** `useMyUsage(range)` → `Usage`; `useMySessions(page)` → `SessionPage` (all time; the API has no range filter on sessions).

#### Plan windows (LimitStrip)

- Source: `usage.limits` (latest snapshot per account the user used in range). Account name: `usage.by_account.find(r => r.key === String(l.account_id))?.label`, else "Account {id}". Plan chip: from the first loaded session whose `account.id === l.account_id`; omit the chip if none is loaded.
- One panel (`--surface`, `--hair`, `--r-panel`, padding `--s-4 --s-5`); one block per account separated by `--hair`. Two LimitMeters per block (5-hour, 7-day). Desktop: label column 64px, meter 240px, then figure and status text. Phone: label and figure on one line, meter full width below, status text below that.
- Header line per account: account name (body 600), PlanChip, right-aligned meta "Reported {relative}" from `captured_at`. If older than 3 h: "Reported 5 h ago, may be out of date" in `--text-2`.
- Empty (`limits` empty): the panel shows one line, meta: "No plan-limit readings yet. Claude Code reports them during normal use; they appear after your next sync."
- Section heading "Plan windows" (h2) with a meta explainer on the right: "How much of each account's 5-hour and weekly limit is used. This is what can stop you working."

#### Summary sentence

Serif `--t-sentence`, `--text`, max 34ch per line on desktop (max-width 32em). Figures inside at weight 600, tabular.

- Range phrase: today → "Today", 7d → "In the last 7 days", 30d → "In the last 30 days", 90d → "In the last 90 days".
- `read = input + cache_write_5m + cache_write_1h + cache_read`; `written = output`.
- Text: "{Range phrase}, Claude read {formatTokens(read)} tokens and wrote {formatTokens(written)} for you across {n} {account|accounts}. At pay-as-you-go API prices that would be {costLabel}."
- `n = usage.by_account.length`. If cost is fully unpriced, the second sentence becomes "None of it is priced yet, so there's no API-equivalent figure."
- Under it, meta `--text-3`: "API-equivalent only. The team's Max plans are flat-rate, so nobody pays this." If `unpriced_tokens > 0` append " Excludes {formatTokens(unpriced)} tokens from models without a price."
- Zero usage: "{Range phrase}, there's no Claude Code usage from you yet." Meta: "Usage shows up a minute or two after a session syncs. If you expected some, run `cctrack status` in a terminal." (command in mono).

#### Token mix (TokenMix)

Four figures in one row (wraps to 2×2 below 560px), separated by space, no boxes: label (meta `--text-3`) above, value (`--t-fig`) and share (meta) on one line. Labels: "Written", "New input", "Cache writes", "Cache reads". Share = part / total_tokens via `formatShare` (§4.7). Below, a 6px bar, full width, segments in `--kind-*` order Written → New input → Cache writes → Cache reads, 2px `--bg` gaps, any non-zero segment at least 2px wide, `--r-cell` corners on outer ends. Each figure has a `title`/tooltip with its glossary line (§7). The bar is `aria-hidden`; the figures carry the information.

#### Daily chart

h2 "API-equivalent cost per day, by model". StackedDailyBars (§5) from `pivot(usage.daily_by_model, datesBetween(range), 'cost_usd')` with `markUnpriced`. Hidden when the range is Today (one bar says nothing); replaced by nothing. Footnote (UnpricedNote) when relevant. "Show as table" toggle under the chart (§5.4).

#### Ledger: Latest sessions

h2 "Latest sessions", meta beneath "All time, newest first. Select a session to see what drove its cost." Then SessionLedger (§6.1) with `data = sessions.data`, `showUser = false`. Weight bars scale to the max cost on the current page.

- Loading: 5 skeleton rows (title bar 60% width, summary bar 40%, meta bar 30%).
- Error: Notice (error) "Couldn't load your sessions: {message}." + "Try again".
- Empty: EmptyState "No sessions yet." body: "Sessions appear here after Claude Code syncs through the cctrack plugin. Run `cctrack status` to check you're logged in."

#### Rail: By model, By account

- **By model** (h2): compact table from `usage.by_model`: Model (body), Read (num), Written (num), API-equivalent (num, CostFigure). Sorted by cost desc. Row hover only, not clickable. Empty: "No usage in this range."
- **By account** (h2): same columns from `usage.by_account`, first column "Account".
- At <1100px these become full-width tables.

### 4.2 Session detail (`/sessions/$sessionId`)

**Purpose:** answer "why did this session use what it used".

New route: `createRoute({ getParentRoute: () => appRoute, path: '/sessions/$sessionId', component: SessionDetailPage })`. Hook `useSession(id)`: `queryKey: ['session', id]`, `GET /sessions/{sessionId}`, `refetchInterval: USAGE_REFETCH_MS`.

```
  ‹ My usage
  BITS Pilani SWD website IIS migration                         (display serif)
  Moved the SWD site from IIS to nginx and fixed the redirect rules.   (prose)

  [person] Rishit   [card] DVM admin  Max 20x   [device] rishit-mbp
  [folder] ~/Coding/swd-site ⧉   [branch] main   Mon 13 Oct, 14:05 for 1 h 12 min

  ─────────────────────────────────────────────────────────────────────
  $32.04                 120.3M            48.2k           333            412k
  API-equivalent         read              written         API calls      largest context

  Why it cost this much
  ┌───────────────────────────────────────────────────────────────────┐
  │ Main conversation ▬▬▬▬▬▬▬ │ Subagents ▬▬▬▬▬▬▬▬▬▬▬▬▬▬▬▬▬▬▬▬▬▬▬▬▬▬▬ │  cost split bar, 12px
  └───────────────────────────────────────────────────────────────────┘
                     Main conversation        Subagents
  API-equivalent     $4.90  (15%)             $27.14 (85%)
  API calls          37                       296
  Avg context/call   ~486k                    ~345k
  Read               18.0M                    102.3M
  Written            11.2k                    37.0k

  • Subagents did most of the work: 296 of 333 calls and $27.14 of $32.04.
  • Each subagent call re-read about 345k tokens of context, so many calls
    multiplied a large context: 296 × 345k ≈ 102M tokens read.
  • claude-opus-5-5 accounts for 91% of the API-equivalent cost.

  By model                                         Token mix
  Model          Calls  Read   Written  API-eq.    (TokenMix component)
  ...

  Session id  4f1c…a9e2 ⧉
```

- **Back link**: chevron-left glyph + "My usage" (to `/`) when `session.user.id === me.id`, else "By login" (to `/admin/logins`). Meta size, `--text-3`, hover `--text`.
- **Title block**: `sessionLabel(session)` primary in `--t-display`; secondary (summary) in `--t-prose`, max 68ch. If the primary is a fallback (project/cwd), add meta `--text-3` "No title recorded for this session."
- **Facts** (FactList, wraps, `gap: var(--s-2) var(--s-5)`): person (`user.name || user.email`, admins only see this when it's someone else's session; always show for admin views), account (AccountName + PlanChip), device (`device_hostname`), folder (`shortenPath(cwd)` mono, full path in `title`, CopyButton copies the full `cwd`), branch (mono), time ("{formatDayLong(started_at)}, {HH:mm} for {formatDuration(ended − started)}"). Missing fields are omitted, not dashed.
- **Headline figures**: five figures in a row (`--t-fig-lg` value, meta label below), wrapping to 2 columns below 560px: API-equivalent (CostFigure), Read, Written, API calls (`calls`), Largest context (`peak_context_tokens`, tooltip "The biggest single call: input plus cache reads and writes.").
- **Why it cost this much** (SessionAnatomy):
  - Cost split bar, 12px tall, full width: main segment `--meter-ok`, subagents segment `--weight`, 2px gap, each labelled above its segment start ("Main conversation", "Subagents") in meta; widths by `cost_usd` (fall back to read tokens if both costs are 0). Hidden if `subagents.calls === 0`, and the comparison table becomes a single column titled "Main conversation".
  - Comparison table (rows × two columns, num cells right-aligned): API-equivalent with share in brackets, API calls, Avg context per call (`~` + formatTokens(read/calls), "–" if calls 0), Read, Written.
  - Driver list: `costDrivers(detail)` (§6.3) as a bulleted list in `--t-prose` (`--text`), max 3 items, figures in sans 600.
- **By model** table from `by_model`: Model, Calls, Read, Written, API-equivalent, plus a 64px share bar (`--weight`) of cost. Sorted by cost desc.
- **Token mix**: TokenMix with the session totals.
- **Footer**: meta "Session id" + mono id + CopyButton.
- Loading: skeleton title (2 lines), facts row, figure row.
- 404: EmptyState "Session not found." body "It may belong to someone else, or it hasn't synced yet." + link back.
- Other errors: Notice (error) "Couldn't load this session: {message}." + "Try again".

### 4.3 By login (`/admin/logins`)

**Purpose:** who used how much, on which accounts, and on what work.

```
  Usage by login                                     [Today|7 days|30 days|90 days]
  ───────────────────────────────────────────────────────────────────────────────
  People                                  Accounts ■ DVM admin  ■ Pool 2  ■ Pro 1
  ▸ Rishit Kumar               ▬▬▬▬▬▬▬▬▬▬▬▬▬▬▬▬▬▬▬▬▬▬▬▬▬▬     $31.20   1.1M written
    rishit@…                   (stacked by account)
  ▾ Aanya Shah                 ▬▬▬▬▬▬▬▬▬▬                    $9.80   300k written
    ┊ (her latest sessions: compact ledger, 20 per page, rows link to detail)
  ───────────────────────────────────────────────────────────────────────────────
  Per account and plan                                    (matrix table, as today)
  ───────────────────────────────────────────────────────────────────────────────
  API-equivalent cost per day, by login     (StackedDailyBars, top 5 + Other)
```

- **Data:** `useByLogin(range)`, `useUserSessions(userId, page)` on expand.
- **People list** (replaces the recharts horizontal bar chart): one row per `users[]`, sorted by `totals.cost_usd` desc. Row grid: disclosure button (chevron, name `user.name || user.email` body 600, email meta below) 240px | AccountSplitBar 1fr | cost (`--t-fig`) 96px | "{written} written" meta 120px. AccountSplitBar: HTML bar, 10px tall, total width proportional to this user's cost / max user cost; segments per `by_account` in account colour (`--series-n` by index in `accountColumns(data)`, top 5 then Other), 2px gaps. Hover or focus on a segment shows ChartTooltip: "{account label} ({plan}) — {cost}, {written} written, {read} read". Legend for accounts above the list (swatch + label + PlanChip). Phone: bar moves under the name, figures on the right of the name line.
  - Expand toggles a region (`aria-expanded`, `aria-controls`) containing SessionLedger `variant="compact"` for that user (all time, paged). Only one open at a time (existing behaviour). Heading inside: meta "Latest sessions, all time".
- **Per account and plan**: keep the existing grouped matrix (plan group header row, account columns, plan subtotals). Restyle: header cells meta 500 `--text-3`, plan group header shows PlanChip; cells show CostFigure on line 1 and "{written} written" meta on line 2; empty cells "–" in `--text-3`. First column sticky (`position: sticky; left: 0; background: var(--bg)`) inside the horizontal scroller. Login names in this table link-scroll to the person row and open it.
- **Daily chart**: h2 "API-equivalent cost per day, by login", StackedDailyBars from `daily_by_user`.
- Empty (no users with usage): EmptyState "No usage in this range." body "Pick a longer range, or check that people have logged in with `cctrack login`."

### 4.4 By account (`/admin/accounts`)

**Purpose:** how full each shared account is, when it resets, and who filled it.

```
  Usage by Claude account                            [Today|7 days|30 days|90 days]
  Jump to  DVM admin   Pool 2   Pro 1   (only when >3 accounts; a row of links)
  ───────────────────────────────────────────────────────────────────────────────
  DVM admin  Max 20x   admin@bits-dvm.org                 $63.10   2.4M written
  ┌───────────────────────────────────────────────────────────────────────────┐
  │ 5-hour ▮▮▮▮▮▮▮▮▮▮▮▮▯▯▯▯▯▯▯▯ 62%  Resets in 2 h 14 min     Reported 12 min ago│
  │ 7-day  ▮▮▮▮▮▮▮▮▮▮▮▮▮▮▮▯▯▯▯▯ 74%  Getting full. Resets Mon 20 Oct, 09:00      │
  └───────────────────────────────────────────────────────────────────────────┘
  Plan limits over time                       Who used it
  (LimitTimeline, 200px)                      Rishit   ▬▬▬▬▬▬▬▬▬▬▬▬  62%  $39.10
                                              Aanya    ▬▬▬▬▬         24%  $15.20
                                              ...
  API-equivalent cost per day, by person  (StackedDailyBars, kind bar)
```

- **Data:** `useByAccount(range)`.
- **Order:** accounts sorted by current pressure: `max(live five_hour_pct, live seven_day_pct)` desc (a window whose reset time has passed counts as 0), accounts without snapshots last, ties by cost desc.
- **Section per account** (not a card): separated by `--hair` and `--s-7` spacing. `id="account-{id}"` for anchors.
  - Header line: `accountName(account)` in `--t-h2` but 20px, PlanChip (for `unknown`: chip "Plan not set" with a link "Set plan" to `/admin/settings#account-{id}`), email meta, right side CostFigure + "{written} written" + "{read} read" meta.
  - LimitStrip with the latest snapshot (`limits[limits.length-1]`). No snapshot: one meta line "No plan-limit readings for this account yet."
  - Two columns at ≥860px (`1.4fr 1fr`): **Plan limits over time** (LimitTimeline, §5.3) and **Who used it** (ShareBars: one row per `by_user`, name, bar width = `share`, `formatPct(share)`, CostFigure; rows sorted by share desc; bar colour `--weight`; caption meta "Share of this account's API-equivalent cost in the range (token share when nothing is priced)").
  - **API-equivalent cost per day, by person**: StackedDailyBars from `daily_by_user`. Person colours are stable across accounts: slot by index of the user in the page-wide list of users sorted by total cost across all accounts.
- Empty: EmptyState "No Claude accounts have reported usage yet." body "Accounts appear automatically after someone's Claude Code syncs."

### 4.5 Settings (`/admin/settings`)

**Purpose:** set account labels and plans, manage CLI tokens, see people, edit model prices.

```
  Settings
  ───────────────────────────────────────────────────────────────────────────────
  ┌ ! 2 accounts have no plan set. Set them so everyone can tell which pool  ┐
  │   account is which tier.                               [Show accounts]   │
  └──────────────────────────────────────────────────────────────────────────┘
  Accounts   CLI tokens   People   Model prices      (in-page links; sticky left column ≥1100px)

  Claude accounts
  Accounts appear automatically when someone's Claude Code reports usage.
  Label                      Plan                                     
  [DVM admin            ]    [Max 20x ▾]   Claude reports tier: default_claude_max_20x   [Save]
  admin@bits-dvm.org
  ┃[                     ]   [Plan not set ▾]  Suggested: Max 5x [Use it]              [Save]
  ┃pool2@bits-dvm.org
  ...
```

- Layout: ≥1100px a 180px sticky in-page nav on the left (links to `#accounts`, `#tokens`, `#people`, `#prices`, active by scroll position is optional), sections on the right. Below, the links render as a horizontal row under the page header.
- **Unknown plan Notice** (warning): shown when any account has `plan === 'unknown'`: "{n} {account has|accounts have} no plan set. Set the plan so everyone can tell which pool account is which tier." Button "Show accounts" scrolls to the first unknown row and focuses its plan select.
- **Claude accounts** (`useAccounts`): rows as a form list (not a dense table). Fields: Label input (placeholder `email ?? account_uuid`, `maxLength 100`), identity meta below (email or uuid in mono), Plan select (`PLANS`, `PLAN_LABELS` but `unknown` shown as "Not set"), tier meta "Claude reports tier: {rate_limit_tier}" (mono value; omitted when absent; `org_type` appended as ", {org_type} org"). Rows with `unknown` get a 3px `--meter-high` left border and, when `suggestPlan(rate_limit_tier)` returns a plan, "Suggested: {label}" with a small "Use it" button that sets the draft plan. Save button states as today: disabled when clean, "Saving…", then a 2 s inline `--text-3` "Saved" with check glyph. Conflict: Notice (warning) inline "Someone else changed this account." + button "Load their version". Errors: `--crit-text` "Couldn't save: {message}".
- **CLI tokens** (`useTokens`): table User, Device, Created, Last used, action. Active tokens first (by last used desc); revoked ones collapsed behind a disclosure "Show {n} revoked tokens". Revoke uses ConfirmButton (§6.4): "Revoke" → inline "Revoke {device}? Their Claude Code stops working until they log in again." + "Revoke token" (destructive) + "Cancel". Revoked rows show meta "Revoked {date}". Error: "Couldn't revoke: {message}".
- **People** (`useUsers`): table Person (name, email meta, chip "Admin" for admins), Joined, Last seen (relative when <7 days, else date). The person name links to `/admin/logins` with that person expanded (pass `?open={id}` search param; By login reads it once).
- **Model prices** (`usePrices`): heading "Model prices", meta "USD per million tokens. Used only for the API-equivalent figures, and changes apply to all past usage. Add a model that shows as unpriced." Table: Model (mono), Input, Cache write 5m, Cache write 1h, Cache read, Output, Save. Rate inputs 88px, right-aligned, tabular. Add row in `tfoot` unchanged in behaviour; copy "Add model". Fast/long tiers shown as meta under the model id: "Also has fast-mode and long-context rates (kept as is)."
- Loading per section: 3 skeleton rows. Error per section: Notice (error) "Couldn't load {section}: {message}." + "Try again".

### 4.6 Login (`/login`)

```
  (vertically centred, left-aligned block, max-width 30rem, at 18vw from left on desktop; centred column on phone)

  cctrack
  What your team's Claude Code
  sessions were spent on.               (serif 34/40, --text)

  Plan windows, API-equivalent cost and the work
  behind it, per person and per shared account.   (body, --text-2)

  [  Sign in with Google  ]             (primary button)
  Only team members your admin has allowed can sign in.   (meta)
```

- Primary button: height 44px, `--accent` fill, `--on-accent` text, sans 600 15px, radius `--r-ctl`, hover `filter: brightness(1.08)` (none in reduced motion is fine; it's not motion). It is the existing `<a href="/api/auth/web/start">`.
- No illustration, no card.

### 4.7 Formatting rules (shared, in `lib/format.ts`)

| Function | Rule | Examples |
|---|---|---|
| `formatTokens` | existing | 950, 18.2k, 1.4M, 2.1B |
| `formatUSD` | existing; plus `> 0 && < 0.005` → "<$0.01" | $0.42, $32.04, $140 |
| `formatShare(part, whole)` | 0 → "0%"; `<0.5%` → "<1%"; else rounded | 96%, <1% |
| `formatTime(iso)` | `hour: '2-digit', minute: '2-digit'` in locale | 14:05 / 2:05 pm |
| `formatDayGroup(iso, now)` | "Today", "Yesterday", else `weekday: 'short', day: 'numeric', month: 'short'`, plus year if not current | Mon 13 Oct |
| `formatDayLong(iso)` | weekday short, day, month short, year if not current | Mon 13 Oct |
| `formatDuration(ms)` | <60 s "under a minute"; <60 min "{m} min"; <24 h "{h} h {m} min" (drop "0 min"); else "{d} d {h} h" | 1 h 12 min |
| `formatRelative(iso, now)` | <1 min "just now"; <60 min "{m} min ago"; <24 h "{h} h ago"; else formatDayLong | 12 min ago |
| `formatReset(iso, now)` | past: handled by LimitMeter; <24 h "Resets in {formatDuration}"; else "Resets {formatDayLong}, {formatTime}" | Resets in 2 h 14 min |
| `formatDayTick(ymd)` | parse as local date (not UTC), `day: 'numeric', month: 'short'` | 13 Oct |
| `readTokens(t)` | `input + cache_write_5m + cache_write_1h + cache_read` | |
| `shortenPath(cwd)` | replace `^/Users/[^/]+` or `^/home/[^/]+` with `~`; if longer than 44 chars keep `~` or first segment + "…/" + last two segments | ~/…/Coding/dvm-merch |

---

## 5. Charts (recharts)

### 5.1 Common rules

- Colours come from `useChartColors()`, which reads the CSS custom properties with `getComputedStyle(document.documentElement)` and re-reads on a `matchMedia('(prefers-color-scheme: dark)')` change event. Pass resolved hex to recharts props.
- Grid: `<CartesianGrid vertical={false} stroke={grid} />` (solid, no dash). Axes: `axisLine={false} tickLine={false}`, tick `{ fill: text3, fontSize: 12, fontFamily: 'Public Sans' }`, Y width 52.
- Legend: never recharts' `<Legend>`; render ChartLegend (HTML, above the chart, left-aligned, wraps): 10×10 swatch `--r-cell` + label in meta `--text-2`. Unpriced series keep the existing "(unpriced)" suffix.
- Tooltip: `<Tooltip content={<ChartTooltip … />} cursor={{ fill: surface2 }} />`. ChartTooltip: `--surface`, `--hair`, `--r-ctl`, `--shadow-pop`, padding `--s-3`, min-width 200px. Header: day in body 600 ("Mon 13 Oct"). Rows: swatch, label (`--text-2`), value right-aligned tabular (`--text`); rows with 0 omitted; a total row with a hairline above, label "Total". Values for cost use the existing `cellCost` text so unpriced cells never read "$0.00".
- Every chart has a "Show as table" toggle (text button, meta size) beneath it that swaps in a DataTable of the same rows (dates × series plus total). This is the accessibility fallback and the relief for low-contrast light-mode slots.
- `isAnimationActive={false}` everywhere. Charts get `role="img"` and an `aria-label` summarising: "API-equivalent cost per day by model, 1 Oct to 15 Oct. Highest day 9 Oct, $6.20."

### 5.2 StackedDailyBars

`BarChart` with one `<Bar stackId="a">` per key (top 5 by total + Other), `fill` series colour, `stroke={bg} strokeWidth={1}` (gives the 2px visual gap between stacked segments), `maxBarSize={28}`, `barCategoryGap="20%"`. Height 220px (180 below 560px). X: `dataKey="date"`, `tickFormatter={formatDayTick}`, `minTickGap={24}`. Y: `tickFormatter` → `$0`, `$5`, `$10` (no decimals when the max ≥ 10, else `$0.50` steps). Used for: by model (My usage), by login (By login), by person (By account). Area charts are no longer used: daily totals are discrete.

### 5.3 LimitTimeline

`LineChart`, height 200. X numeric time as today (`scale="time"`, `formatTimeTick`). Y `domain={[0, 100]}`, ticks `[0, 50, 70, 90, 100]`, `tickFormatter={v => v + '%'}`.

- 5-hour window: `Line type="stepAfter"`, `stroke={series1}`, `strokeWidth={2}`, `dot={false}`, `activeDot={{ r: 4 }}`. 7-day window: same with `series3`.
- Threshold guides: `ReferenceArea y1={90} y2={100} fill={meterCrit} fillOpacity={0.08}` and `ReferenceLine y={70} stroke={lineStrong} strokeDasharray="2 4"`. No labels (the Y ticks name them).
- Reset markers from `limitTimeline().resets` (existing thinning): `ReferenceLine x` with the series colour at 50% opacity, `strokeDasharray="2 4"`, no text label; the tooltip lists resets within ±15 min of the hovered time as "5-hour window reset at 14:00".
- Tooltip header: `formatDateTime`; rows "5-hour window 62%", "7-day window 74%".
- ChartLegend above: "5-hour window", "7-day window", plus a dashed swatch "Window reset".
- Fewer than 2 snapshots: meta "Not enough plan-limit readings in this range to draw a timeline."

### 5.4 Non-recharts bars

AccountSplitBar, ShareBars, WeightBar, TokenMix bar and the SessionAnatomy split bar are plain HTML/CSS (divs with `flex` widths). They are faster, theme with CSS variables directly, and keep text in the DOM.

---

## 6. Shared component inventory

Build in `apps/web/src/components/`. Props are TypeScript; "→" describes behaviour.

### 6.1 Session ledger

**`SessionLedger`** `{ data?: SessionPage; error?: Error | null; page: number; onPage(p): void; showUser?: boolean; variant?: 'full' | 'compact'; onRetry?(): void }` → groups `data.items` by `formatDayGroup(started_at)`, renders DayGroup headers and SessionRows, then Pager. Handles loading, error and empty states (§4.1). `maxCost` = max `cost_usd` on the page, passed to rows.

**`DayGroup`** `{ label: string; children }` → `<section aria-labelledby>` with header: label in sans 600 13px `--text-2`, followed by a hairline filling the rest of the line. Header is `position: sticky; top: var(--header-h)` (plus tab row height below 860px) with `background: var(--bg)`.

**`SessionRow`** `{ s: Session; showUser: boolean; maxCost: number; compact?: boolean }` → the whole row is one `<Link to="/sessions/$sessionId" params={{ sessionId: s.session_id }}>` (display: grid; no nested interactive elements).

```
desktop (≥560px)  grid-template-columns: 72px minmax(0,1fr) 128px; column-gap 16px; padding 14px 12px
┌────────┬───────────────────────────────────────────────────────┬──────────────┐
│ 14:05  │ BITS Pilani SWD website IIS migration                 │       $32.04 │
│ 1 h 12m│ Moved the SWD site from IIS to nginx and fixed the…   │  ▬▬▬▬▬▬▬▬▬▬  │
│        │ [person] Rishit [folder] ~/Coding/swd-site [branch] main│  120.3M read │
│        │ [device] rishit-mbp [card] DVM admin (Max 20x)        │ 48.2k written│
└────────┴───────────────────────────────────────────────────────┴──────────────┘
phone (<560px)  single column
  BITS Pilani SWD website IIS migration
  Moved the SWD site from IIS to nginx and fixed the…
  [folder] ~/…/swd-site  [branch] main
  [device] rishit-mbp  [card] DVM admin (Max 20x)
  14:05, 1 h 12 min                                  $32.04
                                       120.3M read, 48.2k written
```

- Time column: `formatTime(started_at)` body `--text`; `formatDuration(ended − started)` meta `--text-3`.
- Title: `sessionLabel(s).primary`, `--t-entry`, `--text`, max 2 lines (`-webkit-line-clamp: 2`). Underlined on row hover.
- Secondary: `sessionLabel(s).secondary`, `--t-prose` `--text-2`, clamp 2 lines (1 line in compact).
- Facts (FactList, meta `--text-3`, `gap: 4px 16px`, wraps), in this order, each omitted when absent: person (`user.name || user.email`, only when `showUser`), folder (`shortenPath(cwd)` mono; if no `cwd`, `project` in sans), branch (mono, truncated at 28 chars with ellipsis), device (`device_hostname`), account (`account.label` + PlanChip small). Each fact: glyph + text, `title` with the full value.
- Numbers column (right-aligned): CostFigure in `--t-fig`; WeightBar under it (`width = cost / maxCost`, track 96px, bar 3px tall `--weight`, anchored right, min 2px if cost > 0); meta "{read} read" and "{written} written".
- Compact variant (By login drill-down): no summary line clamp beyond 1, no device fact, row padding 10px 12px.
- States: hover `background: var(--surface-2)`, radius `--r-ctl`; focus-visible inset ring.
- Accessible name: the Link gets `aria-label` "{primary}, {formatDayLong} {time}, API-equivalent {cost}" so screen readers don't read every fact; facts stay visible.

**`Pager`** `{ page; total; pageSize; onPage }` → "Newer" (disabled on page 0), meta "{from}–{to} of {total}", "Older". Buttons: secondary style. Scroll the ledger's top into view on change.

**`sessionLabel(s)`** (lib) → `{ primary, secondary?, isFallback }`: `title` → primary, `summary` → secondary. No title: `summary` primary. Neither: `project`, then `basename(cwd)`, then "Untitled session"; `isFallback = true`.

### 6.2 Figures and labels

- **`CostFigure`** `{ totals: Totals; size?: 'fig' | 'fig-lg' | 'body' }` → `costLabel(totals)`. Partly unpriced: the figure followed by a superscript "+" in `--text-3` whose `title` is `unpricedTitle()`; whole element `aria-label` "API-equivalent {cost}, excludes {n} unpriced tokens". Fully unpriced: "Not priced" in `--text-3`.
- **`Tokens`** `{ totals: Totals; layout: 'stacked' | 'inline' }` → "{read} read" and "{written} written".
- **`TokenMix`** `{ totals: Totals }` → §4.1.
- **`PlanChip`** `{ plan: Plan; size?: 'sm' | 'md'; settingsLink?: number }` → pill-less chip: radius `--r-ctl`, 1px `--line` border, `--t-micro` 500, padding 1px 6px, text `PLAN_LABELS[plan]` (`--text-2`). `unknown` → "Plan not set", `--warn-text` on `--warn-wash`, border `--meter-high`; when `settingsLink` is given and the viewer is admin, followed by a link "Set plan".
- **`AccountName`** `{ account: AccountRef | Account }` → label (or email/uuid via `accountName`).
- **`Fact`** `{ glyph; children; title?; mono? }` and **`FactList`** → inline-flex items, glyph `--text-3`.
- **`CopyButton`** `{ value: string; label: string }` → 24px icon button (copy glyph), `aria-label="Copy {label}"`; on success swaps to check glyph for 1.5 s and announces "Copied" via a polite live region.
- **`Glyph`** `{ name }` → §2.4.

### 6.3 Limits and drivers

- **`LimitMeter`** `{ title: '5-hour window' | '7-day window'; pct?: number; resetsAt?: string; now: number; size?: 'md' | 'lg' }` → 20 cells (`md`: 10px tall, cell gap 2px, cells flex 1, max meter width 240px; `lg`: 14px, 320px). Filled cells = `pct > 0 ? max(1, round(pct / 5)) : 0`, coloured by `limitLevel(pct)`: `<70` ok, `70–89` high, `≥90` crit. Figure `Math.round(pct)%` in `--t-fig` (`lg`: `--t-fig-lg`). Status text (meta): high → alert glyph + "Getting full." in `--warn-text`; crit → alert glyph + "Nearly full." in `--crit-text`; then `formatReset`. Reset time in the past: all cells empty with a 1px dashed `--line-strong` outline, figure "–", text "Reset at {formatTime}. Was {pct}% before." `pct` undefined: render nothing. Container `role="meter"` `aria-valuemin=0 aria-valuemax=100 aria-valuenow={pct}` `aria-label="{title}"` `aria-valuetext="{pct}% used, {status}, {reset text}"`.
- **`LimitStrip`** `{ entries: { name: string; plan?: Plan; accountId: number; limit: LimitSnapshot }[]; now: number; size? }` → §4.1 panel.
- **`useNow()`**: the minute clock already in `AppLayout`; expose `now` through `RangeContext` (`RangeState.now: number`) so countdowns update each minute without new timers.
- **`costDrivers(detail: SessionDetail): string[]`** (lib, pure, unit-tested). Let `cost = session.totals.cost_usd`, `read(x) = readTokens(x.totals)`, `avg(x) = read(x) / x.calls`. Evaluate in order, keep the first 3 that fire:
  1. `subagents.calls > 0 && subagents.totals.cost_usd >= 0.5 * cost` → "Subagents did most of the work: {sub.calls} of {calls} calls and {sub cost} of {cost}."
  2. For the bigger of main/subagents by cost, `avg >= 150_000` → "Each {subagent call|call in the main conversation} re-read about {avg} tokens of context, so many calls multiplied a large context: {calls} × {avg} ≈ {read} tokens read."
  3. `calls >= 200` (and rule 2 didn't fire) → "{calls} API calls. Every tool use is a separate call that re-reads the conversation."
  4. `(cache_write_5m + cache_write_1h) >= 0.15 * read(session)` → "{writes} tokens were written to cache. That happens when context changes or the cache expires after a pause (5 minutes or 1 hour)."
  5. Top `by_model` entry has `>= 0.8` of cost and `by_model.length > 1`, or exactly one model → "{model} accounts for {pct} of the API-equivalent cost." (single model: "All calls used {model}.")
  6. Nothing fired → "Nothing unusual: {calls} calls at about {avg} tokens of context each."
  Use `formatTokens`, `formatUSD`, `formatPct`. If `cost === 0` (unpriced), rules 1 and 5 compare read tokens instead and say "tokens read" not cost.
- **`suggestPlan(tier?: string): Plan | undefined`** (lib) → `/max_20x|20x/i` → `max_20x`; `/max_5x|5x/i` → `max_5x`; `/pro/i` → `pro`; else undefined.

### 6.4 General UI

- **`PageHeader`** `{ title: string; range?: boolean; children? }` → §3.3.
- **`RangeControl`** → §3.3.
- **`UserMenu`** → §3.1.
- **`Section`** `{ id?; title: string; description?: ReactNode; actions?: ReactNode; children }` → `<section>` with h2 row (title left, actions right), description meta below (max 68ch), `margin-top: var(--s-7)`.
- **`Notice`** `{ tone: 'info' | 'warning' | 'error'; children; action? }` → `--r-panel`, padding `--s-3 --s-4`, `--hair` border; warning on `--warn-wash` with alert glyph in `--warn-text`; error on `--crit-wash` with `--crit-text`; info on `--surface`. `role="status"` (info/warning) or `role="alert"` (error).
- **`EmptyState`** `{ title; children?; action? }` → left-aligned, padding `--s-6 0`, title body 600, body meta `--text-2`. No illustrations.
- **`Skeleton`** `{ width; height; lines? }` → `--surface-2` blocks, `--r-cell`, opacity pulse 0.55↔1 over 1.2 s (static under reduced motion), `aria-hidden`; parent region sets `aria-busy="true"`.
- **`Button`** variants: `primary` (accent fill), `secondary` (transparent, 1px `--line-strong`, `--text`), `ghost` (no border, `--text-2`, hover `--surface-2`), `danger` (transparent, `--crit-text`, 1px border `--crit-text`). Height 32px (44px on login), padding 0 12px, radius `--r-ctl`, sans 500 14px. Disabled: opacity 0.5, `cursor: not-allowed`.
- **`ConfirmButton`** `{ label; confirmLabel; prompt: string; onConfirm; pending? }` → first click replaces the button with the prompt text + danger button + "Cancel"; focus moves to Cancel; Escape cancels. Replaces `window.confirm`.
- **Inputs/selects**: height 32px, `--surface` background, 1px `--line-strong`, radius `--r-ctl`, sans 15px; invalid: border `--crit-text` and message below in `--crit-text` meta, linked with `aria-describedby`.
- **`DataTable`** styles (class `.table`): header cells meta 500 `--text-3`, bottom `--hair`; body cells body, `padding: 8px 12px`, bottom `--hair`; `.num` right-aligned tabular; wrapper `.table-scroll` keeps horizontal scroll.
- **Charts:** `StackedDailyBars` `{ pivot: Pivot; format; colorFor(key, index): string; ariaLabel: string }`, `LimitTimeline` `{ limits }`, `ChartLegend` `{ items: { label; color; dashed? }[] }`, `ChartTooltip`, `ShareBars` `{ rows: { label; share; totals }[] }`, `AccountSplitBar` `{ parts: { key; label; plan; totals; color }[]; scale: number }`, `WeightBar` `{ value; max }`, `useChartColors()`.

### 6.5 Files

- `index.html`: font links, color-scheme meta.
- `styles.css`: rewritten around §2 tokens; remove `.card`, `.kpis`, `.grid2`, `.gauge` classes.
- `components/`: `Layout.tsx` (shell, PageHeader, RangeControl, UserMenu), `ledger.tsx`, `figures.tsx`, `limits.tsx`, `charts.tsx`, `ui.tsx` (Button, Notice, EmptyState, Skeleton, ConfirmButton, CopyButton, Glyph, Section).
- `pages/SessionDetail.tsx` (new), router entry, `useSession` in `api/hooks.ts`.
- `lib/format.ts` additions (§4.7), `lib/sessions.ts` (`sessionLabel`, `costDrivers`, `shortenPath`, `readTokens`), `lib/accounts.ts` (`suggestPlan`), with unit tests alongside the existing ones.

---

## 7. Copy glossary

Use these exact strings. Tooltips (`title`) and the "What these mean" disclosure at the foot of My usage (a ghost button that expands a definition list of the entries below) both draw from this table.

| Term | Label in UI | Explanation (one line) |
|---|---|---|
| API-equivalent cost | "API-equivalent" (column/figure label); "API-equivalent cost" in headings | What this usage would cost at pay-as-you-go API prices. The team's Max plans are flat-rate, so nobody pays this; use it to compare sessions. |
| Read | "read" (after a number), "Read" (column) | Everything sent to Claude on each call: your messages, files, tool results and the conversation so far. Most of it comes from cache. |
| Written | "written", "Written" | What Claude produced: replies, code and tool calls. |
| New input | "New input" | Context sent for the first time and not cached. |
| Cache writes | "Cache writes" | Context saved to the cache so later calls can reuse it. Priced a little above new input. |
| Cache reads | "Cache reads" | Context reused from the cache on later calls. Billed at about a tenth of the input price, so it dominates token counts but not cost. |
| API calls | "API calls" | Requests Claude Code made to the model. Each tool use is a separate call. |
| Context per call | "Avg context per call" | Tokens read per call on average. Large contexts make every call heavier. |
| Largest context | "Largest context" | The biggest single call in the session: input plus cache reads and writes. |
| Main conversation | "Main conversation" | Calls made by the session you talked to directly. |
| Subagents | "Subagents" | Calls made by agents the session started (for example parallel reviewers or explorers). |
| Plan windows | "Plan windows" (heading) | How much of each account's 5-hour and weekly limit is used. This is what can stop you working. |
| 5-hour window | "5-hour window" | Usage limit that resets 5 hours after it starts. |
| 7-day window | "7-day window" | Weekly usage limit across all models. |
| Getting full | "Getting full." | 70% or more of the window is used. |
| Nearly full | "Nearly full." | 90% or more is used; work may be blocked until the reset. |
| Window reset | "Reset at {time}. Was {pct}% before." | The window has reset since the last reading; new usage hasn't been reported yet. |
| Reported | "Reported {relative}" | When Claude Code last reported this reading. |
| Plan not set | "Plan not set" | An admin hasn't recorded which plan this account is on. |
| Unpriced | "Not priced", "+" marker | Tokens from models with no price in Settings; they are left out of the API-equivalent figure. |
| Session title | (primary label) | Claude Code's name for the session, set by you or generated. |
| Summary | (secondary label) | A one- or two-line note of the work, written at the end of the session. |

Action labels: "Sign in with Google", "Sign out", "Save", "Saved", "Saving…", "Revoke", "Revoke token", "Cancel", "Add model", "Use it", "Set plan", "Show accounts", "Show as table", "Hide table", "Try again", "Newer", "Older", "Copy folder path", "Copy session id", "Load their version".

Error pattern: "Couldn't {verb} {thing}: {server message}." followed by a "Try again" action where a retry makes sense. Never apologise, never "Something went wrong".

---

## 8. Acceptance checklist

- No element shows `total_tokens` as a standalone headline; every token figure is read/written or a named kind.
- Every session row on every page links to `/sessions/$sessionId`; the detail page renders drivers from `costDrivers`.
- "API-equivalent" appears wherever cost does, and the flat-rate explanation is visible on My usage without hovering.
- Plan windows render above all usage figures on My usage and in each account section on By account.
- Unknown plans show the "Plan not set" chip everywhere a plan appears, and admins get the Settings notice, nav dot and suggested plan.
- Dark and light both pass: text ≥4.5:1, meter fills, weight bars and input borders ≥3:1 (values in §2.1 are pre-checked).
- 390px wide: no horizontal page scroll (tables scroll inside `.table-scroll`), tab row scrolls, ledger rows use the phone layout.
- Keyboard: range radiogroup arrows, user menu Escape/return focus, disclosures with `aria-expanded`, ConfirmButton focus handling, visible focus on every interactive element.
- `prefers-reduced-motion: reduce` removes the meter stagger, transitions and skeleton pulse.
- No new npm packages; fonts via the Google Fonts `<link>` only; icons are the inline SVGs in §2.4.
