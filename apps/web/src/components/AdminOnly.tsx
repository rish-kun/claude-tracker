import { Link } from '@tanstack/react-router'
import type { ReactNode } from 'react'
import { useMe } from '../api/hooks'
import { Notice } from './ui'

export function AdminOnly({ children }: { children: ReactNode }) {
  const me = useMe()
  if (!me.data?.is_admin) {
    return (
      <Notice tone="info" action={<Link to="/">Go to my usage</Link>}>
        This page is for admins. Ask a team admin if you need access.
      </Notice>
    )
  }
  return <>{children}</>
}
