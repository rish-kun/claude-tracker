package auth

import (
	"crypto/rand"
	"crypto/sha256"
	"encoding/base64"
)

const TokenPrefix = "cct_"

// RandomString returns nBytes of crypto randomness, base64url-encoded.
func RandomString(nBytes int) string {
	b := make([]byte, nBytes)
	_, _ = rand.Read(b) // never fails on supported platforms
	return base64.RawURLEncoding.EncodeToString(b)
}

// NewToken returns a new API token and its sha256 hash for storage.
func NewToken() (plain string, hash []byte) {
	plain = TokenPrefix + RandomString(32)
	return plain, HashToken(plain)
}

func HashToken(s string) []byte {
	h := sha256.Sum256([]byte(s))
	return h[:]
}
