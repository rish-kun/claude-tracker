export function LoginPage() {
  return (
    <div className="login">
      <div className="card">
        <h1>cctrack</h1>
        <p className="muted">Claude Code usage for your team.</p>
        <a className="button primary" href="/api/auth/web/start">
          Sign in with Google
        </a>
      </div>
    </div>
  )
}
