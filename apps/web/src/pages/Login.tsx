export function LoginPage() {
  return (
    <main className="login">
      <div className="login-block">
        <span className="wordmark">cctrack</span>
        <h1>What your team's Claude Code sessions were spent on.</h1>
        <p className="login-lede">Plan windows, API-equivalent cost and the work behind it, per person and per shared account.</p>
        <div className="login-cta">
          <a className="btn btn-primary btn-lg" href="/api/auth/web/start">
            Sign in with Google
          </a>
        </div>
        <p className="login-note">Only team members your admin has allowed can sign in.</p>
      </div>
    </main>
  )
}
