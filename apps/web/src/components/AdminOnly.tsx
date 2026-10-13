import type { ReactNode } from 'react'
import { useMe } from '../api/hooks'

export function AdminOnly({ children }: { children: ReactNode }) {
  const me = useMe()
  if (!me.data?.is_admin) return <div className="card">Admins only.</div>
  return <>{children}</>
}
