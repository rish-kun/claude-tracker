package config

import (
	"strings"
	"testing"
)

func env(m map[string]string) func(string) string {
	return func(k string) string { return m[k] }
}

func valid() map[string]string {
	return map[string]string{
		"DATABASE_URL":         "postgres://x",
		"GOOGLE_CLIENT_ID":     "id",
		"GOOGLE_CLIENT_SECRET": "secret",
		"SESSION_SECRET":       strings.Repeat("s", 32),
		"PUBLIC_URL":           "https://cctrack.example.com/",
		"ALLOWED_DOMAINS":      " Example.com , ,acme.io",
		"ADMIN_EMAILS":         "Boss@Example.com",
	}
}

func TestLoadValid(t *testing.T) {
	c, err := Load(env(valid()))
	if err != nil {
		t.Fatal(err)
	}
	if c.PublicURL != "https://cctrack.example.com" {
		t.Errorf("PublicURL = %q", c.PublicURL)
	}
	if strings.Join(c.AllowedDomains, ",") != "example.com,acme.io" {
		t.Errorf("AllowedDomains = %v", c.AllowedDomains)
	}
	if c.AdminEmails[0] != "boss@example.com" {
		t.Errorf("AdminEmails = %v", c.AdminEmails)
	}
	if c.Port != "8080" || !c.SecureCookies() || c.AutoMigrate {
		t.Errorf("defaults wrong: %+v", c)
	}
	if c.MigrateURL() != "postgres://x" {
		t.Errorf("MigrateURL = %q", c.MigrateURL())
	}
}

func TestLoadUnpooledMigrateURL(t *testing.T) {
	m := valid()
	m["DATABASE_URL_UNPOOLED"] = "postgres://direct"
	c, err := Load(env(m))
	if err != nil {
		t.Fatal(err)
	}
	if c.MigrateURL() != "postgres://direct" {
		t.Errorf("MigrateURL = %q", c.MigrateURL())
	}
}

func TestLoadMissing(t *testing.T) {
	_, err := Load(env(map[string]string{}))
	if err == nil {
		t.Fatal("expected error")
	}
	for _, name := range []string{"DATABASE_URL", "GOOGLE_CLIENT_ID", "GOOGLE_CLIENT_SECRET", "SESSION_SECRET", "PUBLIC_URL"} {
		if !strings.Contains(err.Error(), name) {
			t.Errorf("error %q does not mention %s", err, name)
		}
	}
}

func TestLoadShortSecret(t *testing.T) {
	m := valid()
	m["SESSION_SECRET"] = "short"
	if _, err := Load(env(m)); err == nil || !strings.Contains(err.Error(), "SESSION_SECRET") {
		t.Fatalf("expected SESSION_SECRET error, got %v", err)
	}
}

func TestLoadRequiresAllowlist(t *testing.T) {
	m := valid()
	delete(m, "ALLOWED_DOMAINS")
	if _, err := Load(env(m)); err == nil {
		t.Fatal("expected error when no allowlist is configured")
	}
	m["ALLOWED_EMAILS"] = "solo@gmail.com"
	if _, err := Load(env(m)); err != nil {
		t.Fatalf("ALLOWED_EMAILS alone should be enough: %v", err)
	}
}

func TestLoadBadPublicURL(t *testing.T) {
	m := valid()
	m["PUBLIC_URL"] = "cctrack.example.com"
	if _, err := Load(env(m)); err == nil {
		t.Fatal("expected error for PUBLIC_URL without scheme")
	}
}
