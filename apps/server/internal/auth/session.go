package auth

import (
	"crypto/hmac"
	"crypto/sha256"
	"encoding/base64"
	"errors"
	"fmt"
	"strconv"
	"strings"
	"time"
)

var ErrBadSession = errors.New("invalid session")

// SessionCodec signs stateless session cookies: "<userID>.<expUnix>.<hmac>".
type SessionCodec struct {
	secret []byte
}

func NewSessionCodec(secret []byte) SessionCodec { return SessionCodec{secret: secret} }

func (c SessionCodec) sign(payload string) string {
	m := hmac.New(sha256.New, c.secret)
	m.Write([]byte(payload))
	return base64.RawURLEncoding.EncodeToString(m.Sum(nil))
}

func (c SessionCodec) Encode(userID int64, exp time.Time) string {
	payload := fmt.Sprintf("%d.%d", userID, exp.Unix())
	return payload + "." + c.sign(payload)
}

func (c SessionCodec) Decode(value string, now time.Time) (int64, error) {
	parts := strings.Split(value, ".")
	if len(parts) != 3 {
		return 0, ErrBadSession
	}
	payload := parts[0] + "." + parts[1]
	if !hmac.Equal([]byte(c.sign(payload)), []byte(parts[2])) {
		return 0, ErrBadSession
	}
	uid, err1 := strconv.ParseInt(parts[0], 10, 64)
	exp, err2 := strconv.ParseInt(parts[1], 10, 64)
	if err1 != nil || err2 != nil || now.Unix() >= exp {
		return 0, ErrBadSession
	}
	return uid, nil
}
