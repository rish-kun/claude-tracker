// Package auth contains authentication primitives: Google identity, allowlist,
// API tokens, PKCE and signed session cookies.
package auth

import "context"

type Identity struct {
	Subject       string
	Email         string
	Name          string
	HostedDomain  string // Google "hd" claim; empty for consumer accounts
	EmailVerified bool
}

type IdentityProvider interface {
	AuthCodeURL(state, redirectURL string) string
	Exchange(ctx context.Context, code, redirectURL string) (Identity, error)
}
