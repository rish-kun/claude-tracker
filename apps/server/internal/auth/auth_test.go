package auth

import (
	"strings"
	"testing"
	"time"
)

func TestAllowlist(t *testing.T) {
	a := NewAllowlist([]string{"example.com"}, []string{"solo@gmail.com"})
	cases := []struct {
		id   Identity
		want bool
	}{
		{Identity{Email: "a@example.com", HostedDomain: "example.com", EmailVerified: true}, true},
		{Identity{Email: "a@example.com", HostedDomain: "", EmailVerified: true}, false}, // consumer account spoofing a domain
		{Identity{Email: "Solo@Gmail.com", EmailVerified: true}, true},
		{Identity{Email: "solo@gmail.com", EmailVerified: false}, false},
		{Identity{Email: "x@other.com", HostedDomain: "other.com", EmailVerified: true}, false},
		{Identity{Email: "a@EXAMPLE.com", HostedDomain: "Example.com", EmailVerified: true}, true},
	}
	for i, c := range cases {
		if got := a.Allows(c.id); got != c.want {
			t.Errorf("case %d: Allows(%+v) = %v, want %v", i, c.id, got, c.want)
		}
	}
}

func TestNewTokenAndHash(t *testing.T) {
	plain, hash := NewToken()
	if !strings.HasPrefix(plain, TokenPrefix) || len(plain) < 40 {
		t.Fatalf("bad token %q", plain)
	}
	if string(HashToken(plain)) != string(hash) || len(hash) != 32 {
		t.Fatal("hash mismatch")
	}
	other, _ := NewToken()
	if other == plain {
		t.Fatal("tokens must be random")
	}
}

func TestVerifyPKCE(t *testing.T) {
	// RFC 7636 Appendix B
	verifier := "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"
	challenge := "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
	if !VerifyPKCE(verifier, challenge) {
		t.Fatal("RFC vector should verify")
	}
	if VerifyPKCE(verifier+"x", challenge) {
		t.Fatal("wrong verifier must fail")
	}
	if VerifyPKCE("short", challenge) {
		t.Fatal("too-short verifier must fail")
	}
}

func TestSessionCodec(t *testing.T) {
	c := NewSessionCodec([]byte(strings.Repeat("k", 32)))
	now := time.Unix(1_800_000_000, 0)
	v := c.Encode(42, now.Add(time.Hour))
	uid, err := c.Decode(v, now)
	if err != nil || uid != 42 {
		t.Fatalf("Decode = %d, %v", uid, err)
	}
	if _, err := c.Decode(v, now.Add(2*time.Hour)); err == nil {
		t.Fatal("expired cookie must fail")
	}
	if _, err := c.Decode(strings.Replace(v, "42.", "43.", 1), now); err == nil {
		t.Fatal("tampered cookie must fail")
	}
	other := NewSessionCodec([]byte(strings.Repeat("z", 32)))
	if _, err := other.Decode(v, now); err == nil {
		t.Fatal("cookie signed with another secret must fail")
	}
	if _, err := c.Decode("garbage", now); err == nil {
		t.Fatal("garbage must fail")
	}
}
