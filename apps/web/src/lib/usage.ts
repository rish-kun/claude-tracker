import type { Totals } from '../api/client'
import { costLabel, formatTokens } from './format'
import { type Preset, RANGE_PHRASE } from './range'
import { readTokens } from './tokens'

/** A run of sentence text; `fig` runs are figures, set heavier and tabular. */
export type Run = string | { fig: string }

export interface UsageSentence {
  runs: Run[]
  /** Meta line under the sentence. `cmd` marks a literal command, set in mono. */
  note: (string | { cmd: string })[]
}

export function runsText(runs: (string | { fig: string } | { cmd: string })[]): string {
  return runs.map((r) => (typeof r === 'string' ? r : 'fig' in r ? r.fig : r.cmd)).join('')
}

/** The My usage summary sentence (§4.1) as runs, so figures can be styled. */
export function usageSentence(preset: Preset, totals: Totals, accounts: number): UsageSentence {
  const phrase = RANGE_PHRASE[preset]
  const read = readTokens(totals)
  const written = totals.output
  if (read + written === 0 && totals.cost_usd === 0) {
    return {
      runs: [`${phrase}, there's no Claude Code usage from you yet.`],
      note: ['Usage shows up a minute or two after a session syncs. If you expected some, run ', { cmd: 'cctrack status' }, ' in a terminal.'],
    }
  }
  const runs: Run[] = [
    `${phrase}, Claude read `,
    { fig: formatTokens(read) },
    ' tokens and wrote ',
    { fig: formatTokens(written) },
    ' for you across ',
    { fig: String(accounts) },
    ` ${accounts === 1 ? 'account' : 'accounts'}. `,
  ]
  if (totals.cost_usd === 0 && totals.unpriced_tokens > 0) {
    runs.push("None of it is priced yet, so there's no API-equivalent figure.")
  } else {
    runs.push('At pay-as-you-go API prices that would be ', { fig: costLabel(totals) }, '.')
  }
  let note = "API-equivalent only. The team's Max plans are flat-rate, so nobody pays this."
  if (totals.unpriced_tokens > 0) note += ` Excludes ${formatTokens(totals.unpriced_tokens)} tokens from models without a price.`
  return { runs, note: [note] }
}
