package auth

import (
	"context"
	"errors"
	"fmt"

	"github.com/coreos/go-oidc/v3/oidc"
	"golang.org/x/oauth2"
)

const googleIssuer = "https://accounts.google.com"

// Google implements IdentityProvider with Google's OAuth 2.0 web flow.
type Google struct {
	clientID     string
	clientSecret string
	verifier     *oidc.IDTokenVerifier
}

// NewGoogle does no network I/O; signing keys are fetched lazily on first verify.
func NewGoogle(ctx context.Context, clientID, clientSecret string) *Google {
	keys := oidc.NewRemoteKeySet(ctx, "https://www.googleapis.com/oauth2/v3/certs")
	return &Google{
		clientID:     clientID,
		clientSecret: clientSecret,
		verifier:     oidc.NewVerifier(googleIssuer, keys, &oidc.Config{ClientID: clientID}),
	}
}

func (g *Google) config(redirectURL string) *oauth2.Config {
	return &oauth2.Config{
		ClientID:     g.clientID,
		ClientSecret: g.clientSecret,
		RedirectURL:  redirectURL,
		Scopes:       []string{"openid", "email", "profile"},
		Endpoint: oauth2.Endpoint{
			AuthURL:   "https://accounts.google.com/o/oauth2/v2/auth",
			TokenURL:  "https://oauth2.googleapis.com/token",
			AuthStyle: oauth2.AuthStyleInParams,
		},
	}
}

func (g *Google) AuthCodeURL(state, redirectURL string) string {
	return g.config(redirectURL).AuthCodeURL(state, oauth2.SetAuthURLParam("prompt", "select_account"))
}

func (g *Google) Exchange(ctx context.Context, code, redirectURL string) (Identity, error) {
	tok, err := g.config(redirectURL).Exchange(ctx, code)
	if err != nil {
		return Identity{}, fmt.Errorf("exchange code: %w", err)
	}
	raw, ok := tok.Extra("id_token").(string)
	if !ok || raw == "" {
		return Identity{}, errors.New("no id_token in token response")
	}
	idt, err := g.verifier.Verify(ctx, raw)
	if err != nil {
		return Identity{}, fmt.Errorf("verify id_token: %w", err)
	}
	var claims struct {
		Sub           string `json:"sub"`
		Email         string `json:"email"`
		EmailVerified bool   `json:"email_verified"`
		Name          string `json:"name"`
		HD            string `json:"hd"`
	}
	if err := idt.Claims(&claims); err != nil {
		return Identity{}, err
	}
	return Identity{
		Subject:       claims.Sub,
		Email:         claims.Email,
		Name:          claims.Name,
		HostedDomain:  claims.HD,
		EmailVerified: claims.EmailVerified,
	}, nil
}
