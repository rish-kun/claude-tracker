import type { Plan } from '../api/client'

/** One-line explanations for the terms the UI uses (§7). Shown as tooltips and in "What these mean". */
export const GLOSSARY = {
  apiEquivalent: {
    term: 'API-equivalent cost',
    text: 'What this usage would cost at pay-as-you-go API list prices; use it to compare sessions.',
  },
  read: {
    term: 'Read',
    text: 'Everything sent to Claude on each call: your messages, files, tool results and the conversation so far. Most of it comes from cache.',
  },
  written: { term: 'Written', text: 'What Claude produced: replies, code and tool calls.' },
  newInput: { term: 'New input', text: 'Context sent for the first time and not cached.' },
  cacheWrites: { term: 'Cache writes', text: 'Context saved to the cache so later calls can reuse it. Priced a little above new input.' },
  cacheReads: {
    term: 'Cache reads',
    text: 'Context reused from the cache on later calls. Billed at about a tenth of the input price, so it dominates token counts but not cost.',
  },
  apiCalls: { term: 'API calls', text: 'Requests Claude Code made to the model. Each tool use is a separate call.' },
  contextPerCall: { term: 'Avg context per call', text: 'Tokens read per call on average. Large contexts make every call heavier.' },
  largestContext: { term: 'Largest context', text: 'The biggest single call in the session: input plus cache reads and writes.' },
  main: { term: 'Main conversation', text: 'Calls made by the session you talked to directly.' },
  subagents: { term: 'Subagents', text: 'Calls made by agents the session started (for example parallel reviewers or explorers).' },
  planWindows: { term: 'Plan windows', text: "How much of each account's 5-hour and weekly limit is used. This is what can stop you working." },
  fiveHour: { term: '5-hour window', text: 'Usage limit that resets 5 hours after it starts.' },
  sevenDay: { term: '7-day window', text: 'Weekly usage limit across all models.' },
  gettingFull: { term: 'Getting full', text: '70% or more of the window is used.' },
  nearlyFull: { term: 'Nearly full', text: '90% or more is used; work may be blocked until the reset.' },
  windowReset: { term: 'Window reset', text: "The window has reset since the last reading; new usage hasn't been reported yet." },
  reported: { term: 'Reported', text: 'When Claude Code last reported this reading.' },
  planNotSet: { term: 'Plan not set', text: "An admin hasn't recorded which plan this account is on." },
  unpriced: { term: 'Not priced', text: 'Tokens from models with no price in Settings; they are left out of the API-equivalent figure.' },
  title: { term: 'Session title', text: "Claude Code's name for the session, set by you or generated." },
  summary: { term: 'Summary', text: 'A one- or two-line note of the work, written at the end of the session.' },
} as const

export type GlossaryKey = keyof typeof GLOSSARY

/** The tooltip text for a term. */
export function explain(k: GlossaryKey): string {
  return GLOSSARY[k].text
}

/**
 * Whether the usage is known to be on flat-rate plans only: `plans` lists the plan of every
 * account in the data, and none is billed per token (`api_key`). Unknown plans (`undefined`
 * or an empty list) don't count as flat-rate, so the UI never claims nobody pays.
 */
export function allFlatRate(plans?: readonly Plan[]): boolean {
  return plans !== undefined && plans.length > 0 && !plans.includes('api_key')
}

/** The API-equivalent tooltip, saying nobody pays it only when every account is on a flat-rate plan. */
export function explainApiEquivalent(plans?: readonly Plan[]): string {
  if (!allFlatRate(plans)) return GLOSSARY.apiEquivalent.text
  return "What this usage would cost at pay-as-you-go API list prices. The team's Max plans are flat-rate, so nobody pays this; use it to compare sessions."
}
