// Package config loads server configuration from environment variables.
package config

import (
	"errors"
	"fmt"
	"strings"
)

type Config struct {
	DatabaseURL         string
	DatabaseURLUnpooled string
	GoogleClientID      string
	GoogleClientSecret  string
	SessionSecret       []byte
	PublicURL           string // no trailing slash
	AllowedDomains      []string
	AllowedEmails       []string
	AdminEmails         []string
	AutoMigrate         bool
	Port                string
}

// SecureCookies reports whether cookies should carry the Secure attribute.
func (c Config) SecureCookies() bool { return strings.HasPrefix(c.PublicURL, "https://") }

// MigrateURL is the connection string used for migrations. Session-level
// advisory locks do not work through a transaction pooler, so prefer the
// direct URL when one is configured.
func (c Config) MigrateURL() string {
	if c.DatabaseURLUnpooled != "" {
		return c.DatabaseURLUnpooled
	}
	return c.DatabaseURL
}

func Load(getenv func(string) string) (Config, error) {
	c := Config{
		DatabaseURL:         strings.TrimSpace(getenv("DATABASE_URL")),
		DatabaseURLUnpooled: strings.TrimSpace(getenv("DATABASE_URL_UNPOOLED")),
		GoogleClientID:      strings.TrimSpace(getenv("GOOGLE_CLIENT_ID")),
		GoogleClientSecret:  strings.TrimSpace(getenv("GOOGLE_CLIENT_SECRET")),
		SessionSecret:       []byte(getenv("SESSION_SECRET")),
		PublicURL:           strings.TrimRight(strings.TrimSpace(getenv("PUBLIC_URL")), "/"),
		AllowedDomains:      list(getenv("ALLOWED_DOMAINS")),
		AllowedEmails:       list(getenv("ALLOWED_EMAILS")),
		AdminEmails:         list(getenv("ADMIN_EMAILS")),
		AutoMigrate:         strings.TrimSpace(getenv("AUTO_MIGRATE")) == "true",
		Port:                strings.TrimSpace(getenv("PORT")),
	}
	if c.Port == "" {
		c.Port = "8080"
	}

	var missing []string
	for _, kv := range [][2]string{
		{"DATABASE_URL", c.DatabaseURL},
		{"GOOGLE_CLIENT_ID", c.GoogleClientID},
		{"GOOGLE_CLIENT_SECRET", c.GoogleClientSecret},
		{"SESSION_SECRET", string(c.SessionSecret)},
		{"PUBLIC_URL", c.PublicURL},
	} {
		if kv[1] == "" {
			missing = append(missing, kv[0])
		}
	}
	if len(missing) > 0 {
		return Config{}, fmt.Errorf("missing required env vars: %s", strings.Join(missing, ", "))
	}

	var errs []error
	if len(c.SessionSecret) < 32 {
		errs = append(errs, errors.New("SESSION_SECRET must be at least 32 bytes"))
	}
	if !strings.HasPrefix(c.PublicURL, "https://") && !strings.HasPrefix(c.PublicURL, "http://") {
		errs = append(errs, errors.New("PUBLIC_URL must start with http:// or https://"))
	}
	if len(c.AllowedDomains) == 0 && len(c.AllowedEmails) == 0 {
		errs = append(errs, errors.New("set ALLOWED_DOMAINS and/or ALLOWED_EMAILS; refusing to allow every Google account"))
	}
	if err := errors.Join(errs...); err != nil {
		return Config{}, err
	}
	return c, nil
}

// list splits a comma-separated env var into trimmed, lowercased, non-empty items.
func list(s string) []string {
	var out []string
	for _, part := range strings.Split(s, ",") {
		part = strings.ToLower(strings.TrimSpace(part))
		if part != "" {
			out = append(out, part)
		}
	}
	return out
}
