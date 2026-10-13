package auth

import (
	"context"
	"encoding/base64"
	"fmt"
	"net/http"
	"net/http/httptest"
	"testing"
	"time"
)

func TestNewGoogleUsesTimeoutClient(t *testing.T) {
	g := NewGoogle(context.Background(), "id", "secret")
	if g.client == nil || g.client.Timeout != googleHTTPTimeout || googleHTTPTimeout != 10*time.Second {
		t.Fatalf("client = %+v, want a %v timeout", g.client, googleHTTPTimeout)
	}
	if g.tokenURL != googleTokenURL {
		t.Fatalf("tokenURL = %q", g.tokenURL)
	}
}

// hangingServer never answers until the client gives up (or the test ends).
func hangingServer(t *testing.T) *httptest.Server {
	t.Helper()
	release := make(chan struct{})
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		select {
		case <-r.Context().Done():
		case <-release:
		}
	}))
	t.Cleanup(func() {
		close(release)
		srv.Close()
	})
	return srv
}

// boundedCall runs fn with a 5s backstop context and asserts that it fails
// well before then, i.e. that the injected client's timeout applied.
func boundedCall(t *testing.T, what string, fn func(ctx context.Context) error) {
	t.Helper()
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	start := time.Now()
	err := fn(ctx)
	if err == nil {
		t.Fatalf("%s against a hanging server succeeded", what)
	}
	if elapsed := time.Since(start); elapsed > 3*time.Second {
		t.Fatalf("%s took %v (err %v); the client timeout was not applied", what, elapsed, err)
	}
}

func TestGoogleExchangeUsesClientTimeout(t *testing.T) {
	srv := hangingServer(t)
	g := newGoogle(context.Background(), "id", "secret", &http.Client{Timeout: 200 * time.Millisecond},
		srv.URL+"/certs", srv.URL+"/token")
	boundedCall(t, "code exchange", func(ctx context.Context) error {
		_, err := g.Exchange(ctx, "code", "https://cctrack.test/api/auth/google/callback")
		return err
	})
}

func TestGoogleKeyFetchUsesClientTimeout(t *testing.T) {
	srv := hangingServer(t)
	g := newGoogle(context.Background(), "id", "secret", &http.Client{Timeout: 200 * time.Millisecond},
		srv.URL+"/certs", srv.URL+"/token")
	b64 := base64.RawURLEncoding.EncodeToString
	payload := fmt.Sprintf(`{"iss":%q,"aud":"id","sub":"s","exp":%d}`, googleIssuer, time.Now().Add(time.Hour).Unix())
	jwt := b64([]byte(`{"alg":"RS256","kid":"k"}`)) + "." + b64([]byte(payload)) + "." + b64([]byte("not-a-signature"))
	boundedCall(t, "signing key fetch", func(ctx context.Context) error {
		_, err := g.verifier.Verify(ctx, jwt)
		return err
	})
}
