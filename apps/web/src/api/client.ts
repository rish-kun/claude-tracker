import createClient, { type Middleware } from 'openapi-fetch'
import type { components, paths } from './schema'

export type Schemas = components['schemas']
export type Me = Schemas['Me']
export type Usage = Schemas['Usage']
export type Totals = Schemas['Totals']
export type BreakdownRow = Schemas['BreakdownRow']
export type SeriesPoint = Schemas['SeriesPoint']
export type LimitSnapshot = Schemas['LimitSnapshot']
export type Session = Schemas['Session']
export type SessionPage = Schemas['SessionPage']
export type SessionDetail = Schemas['SessionDetail']
export type SessionPart = Schemas['SessionPart']
export type ByLogin = Schemas['ByLogin']
export type ByAccount = Schemas['ByAccount']
export type AccountDetail = Schemas['AccountDetail']
export type Account = Schemas['Account']
export type AccountRef = Schemas['AccountRef']
export type Plan = Schemas['Plan']
export type AdminUser = Schemas['AdminUser']
export type ApiToken = Schemas['ApiToken']
export type ModelPrice = Schemas['ModelPrice']
export type PriceRates = Schemas['PriceRates']

export class ApiError extends Error {
  constructor(
    public status: number,
    message: string,
  ) {
    super(message)
  }
}

const csrf: Middleware = {
  onRequest({ request }) {
    request.headers.set('X-CCTrack', '1')
    return request
  },
}

export const api = createClient<paths>({ baseUrl: '/api' })
api.use(csrf)

/** Resolves to the response data or throws ApiError. */
export async function unwrap<R extends { data?: unknown; error?: unknown; response: Response }>(
  p: Promise<R>,
): Promise<NonNullable<R['data']>> {
  const { data, error, response } = await p
  if (!response.ok || error !== undefined) {
    const message = (error as { error?: string } | undefined)?.error ?? response.statusText
    throw new ApiError(response.status, message)
  }
  return data as NonNullable<R['data']>
}
