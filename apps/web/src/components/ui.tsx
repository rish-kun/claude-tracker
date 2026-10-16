import { type ComponentProps, type ReactNode, useEffect, useId, useRef, useState } from 'react'

/* ───────── Glyphs (§2.4): 16×16 line icons, inline SVG, decorative. ───────── */

export type GlyphName =
  | 'folder'
  | 'branch'
  | 'device'
  | 'account'
  | 'person'
  | 'copy'
  | 'check'
  | 'chevron-right'
  | 'chevron-down'
  | 'chevron-left'
  | 'alert'

const PATHS: Record<Exclude<GlyphName, 'alert' | 'branch' | 'person'>, string> = {
  folder: 'M2 4.5h4l1.5 1.5H14v6.5H2z',
  device: 'M3.5 3.5h9v7h-9zM1.5 13h13',
  account: 'M2 4.5h12v8H2zM2 7.5h12',
  copy: 'M5.5 5.5h7v7h-7zM3.5 10.5v-7h7',
  check: 'M3.5 8.5l3 3 6-7',
  'chevron-right': 'M6 3.5 10.5 8 6 12.5',
  'chevron-down': 'M3.5 6 8 10.5 12.5 6',
  'chevron-left': 'M10 3.5 5.5 8 10 12.5',
}

export function Glyph({ name, size = 14, className }: { name: GlyphName; size?: number; className?: string }) {
  const common = {
    className: className ? `glyph ${className}` : 'glyph',
    width: size,
    height: size,
    viewBox: '0 0 16 16',
    'aria-hidden': true,
    focusable: false,
  } as const
  if (name === 'alert') {
    return (
      <svg {...common}>
        <path d="M8 1.8 15 14H1z" fill="currentColor" />
        <path d="M8 6v4M8 11.6v.4" stroke="var(--surface)" strokeWidth={1.5} strokeLinecap="round" fill="none" />
      </svg>
    )
  }
  return (
    <svg {...common} fill="none" stroke="currentColor" strokeWidth={1.5} strokeLinecap="round" strokeLinejoin="round">
      {name === 'branch' ? (
        <>
          <circle cx={4} cy={3.5} r={1.5} />
          <circle cx={4} cy={12.5} r={1.5} />
          <circle cx={12} cy={5} r={1.5} />
          <path d="M4 5v6M12 6.5c0 3-8 2-8 4.5" />
        </>
      ) : name === 'person' ? (
        <>
          <circle cx={8} cy={5.5} r={2.5} />
          <path d="M3 13.5c.8-2.6 2.8-4 5-4s4.2 1.4 5 4" />
        </>
      ) : (
        <path d={PATHS[name]} />
      )}
    </svg>
  )
}

/* ───────── Buttons ───────── */

export type ButtonVariant = 'primary' | 'secondary' | 'ghost' | 'danger'

export function Button({
  variant = 'secondary',
  size,
  className,
  type = 'button',
  ...rest
}: ComponentProps<'button'> & { variant?: ButtonVariant; size?: 'sm' | 'lg' }) {
  const cls = ['btn', `btn-${variant}`, size && `btn-${size}`, className].filter(Boolean).join(' ')
  return <button type={type} className={cls} {...rest} />
}

/* ───────── Notices, empty states, skeletons ───────── */

export function Notice({ tone, children, action }: { tone: 'info' | 'warning' | 'error'; children: ReactNode; action?: ReactNode }) {
  return (
    <div className="notice" data-tone={tone} role={tone === 'error' ? 'alert' : 'status'}>
      {tone !== 'info' && <Glyph name="alert" />}
      <div className="notice-body">{children}</div>
      {action && <div className="notice-action">{action}</div>}
    </div>
  )
}

/** "Couldn't {what}: {message}." with a Try again button when a retry makes sense. */
export function ErrorNotice({ what, error, onRetry }: { what: string; error: Error; onRetry?: () => void }) {
  const msg = error.message.replace(/\.$/, '')
  return (
    <Notice
      tone="error"
      action={
        onRetry && (
          <Button variant="secondary" size="sm" onClick={onRetry}>
            Try again
          </Button>
        )
      }
    >
      Couldn't {what}: {msg}.
    </Notice>
  )
}

export function EmptyState({ title, children, action }: { title: string; children?: ReactNode; action?: ReactNode }) {
  return (
    <div className="empty-state">
      <p className="empty-state-title">{title}</p>
      {children && <p className="empty-state-body">{children}</p>}
      {action && <div className="empty-state-action">{action}</div>}
    </div>
  )
}

export function Skeleton({ width = '100%', height = 14, lines = 1 }: { width?: string | number; height?: number; lines?: number }) {
  return (
    <>
      {Array.from({ length: lines }, (_, i) => (
        <span key={i} className="skel" style={{ width, height }} aria-hidden="true" />
      ))}
    </>
  )
}

/** A few skeleton rows for a loading table or section. */
export function SkeletonRows({ rows = 3 }: { rows?: number }) {
  return (
    <div aria-busy="true" aria-label="Loading" className="mt-3">
      {Array.from({ length: rows }, (_, i) => (
        <div key={i} style={{ padding: '10px 0', borderBottom: 'var(--hair)' }}>
          <Skeleton width={`${70 - i * 12}%`} height={12} />
        </div>
      ))}
    </div>
  )
}

/* ───────── Section ───────── */

export function Section({
  id,
  title,
  description,
  actions,
  className,
  level = 2,
  children,
}: {
  id?: string
  title: string
  description?: ReactNode
  actions?: ReactNode
  className?: string
  /** Heading level: 2 for page sections, 3 inside a titled block (e.g. an account on By account). */
  level?: 2 | 3
  children?: ReactNode
}) {
  const hid = useId()
  const H = level === 3 ? 'h3' : 'h2'
  return (
    <section id={id} className={className ? `section ${className}` : 'section'} aria-labelledby={hid}>
      <div className="section-head">
        <H id={hid}>{title}</H>
        {actions}
      </div>
      {description && <p className="section-desc">{description}</p>}
      {children && <div className="section-body">{children}</div>}
    </section>
  )
}

/* ───────── ConfirmButton: an inline two-step destructive action ───────── */

export function ConfirmButton({
  label,
  confirmLabel,
  prompt,
  onConfirm,
  pending = false,
}: {
  label: string
  confirmLabel: string
  prompt: string
  onConfirm: () => void
  pending?: boolean
}) {
  const [asking, setAsking] = useState(false)
  const cancelRef = useRef<HTMLButtonElement>(null)
  const triggerRef = useRef<HTMLButtonElement>(null)
  const returnFocus = useRef(false)
  useEffect(() => {
    if (asking) cancelRef.current?.focus()
    else if (returnFocus.current) {
      returnFocus.current = false
      triggerRef.current?.focus()
    }
  }, [asking])
  const cancel = () => {
    returnFocus.current = true
    setAsking(false)
  }
  if (!asking) {
    return (
      <Button ref={triggerRef} variant="secondary" size="sm" disabled={pending} onClick={() => setAsking(true)}>
        {pending ? `${label}…` : label}
      </Button>
    )
  }
  return (
    <div
      className="confirm"
      onKeyDown={(e) => {
        if (e.key === 'Escape') {
          e.stopPropagation()
          cancel()
        }
      }}
    >
      <span>{prompt}</span>
      <Button
        variant="danger"
        size="sm"
        onClick={() => {
          setAsking(false)
          onConfirm()
        }}
      >
        {confirmLabel}
      </Button>
      <Button ref={cancelRef} variant="ghost" size="sm" onClick={cancel}>
        Cancel
      </Button>
    </div>
  )
}

/* ───────── CopyButton ───────── */

export function CopyButton({ value, label }: { value: string; label: string }) {
  const [copied, setCopied] = useState(false)
  useEffect(() => {
    if (!copied) return
    const t = setTimeout(() => setCopied(false), 1500)
    return () => clearTimeout(t)
  }, [copied])
  return (
    <>
      <button
        type="button"
        className="btn-icon"
        aria-label={`Copy ${label}`}
        title={`Copy ${label}`}
        onClick={() => {
          void navigator.clipboard?.writeText(value).then(
            () => setCopied(true),
            () => {},
          )
        }}
      >
        <Glyph name={copied ? 'check' : 'copy'} />
      </button>
      <span className="sr-only" role="status" aria-live="polite">
        {copied ? 'Copied' : ''}
      </span>
    </>
  )
}

/* ───────── Disclosure state with a matching region id ───────── */

export function useDisclosure(initial = false) {
  const [open, setOpen] = useState(initial)
  const id = useId()
  return {
    open,
    setOpen,
    buttonProps: { 'aria-expanded': open, 'aria-controls': id, onClick: () => setOpen((o) => !o) },
    regionProps: { id, hidden: !open },
  }
}
