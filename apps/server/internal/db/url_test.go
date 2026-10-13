package db

import (
	"context"
	"strings"
	"testing"
)

func TestCheckMigrateURL(t *testing.T) {
	for _, c := range []struct {
		url    string
		pooled bool
	}{
		{"postgres://u:p@ep-cool-name-123456-pooler.us-east-2.aws.neon.tech/neondb?sslmode=require", true},
		{"postgresql://u:p@EP-X-POOLER.eu-central-1.aws.neon.tech:5432/db", true},
		{"postgres://u:p@ep-cool-name-123456.us-east-2.aws.neon.tech/neondb?sslmode=require&channel_binding=require", false},
		{"postgres://postgres:postgres@localhost:54329/postgres?sslmode=disable", false},
		{"postgres://u:p@/db?host=ep-a-pooler.neon.tech", true},
		{"host=ep-a-pooler.neon.tech user=u dbname=db sslmode=require", true},
		{"host='ep-a.neon.tech' user=u dbname=db", false},
	} {
		err := CheckMigrateURL(c.url)
		if (err != nil) != c.pooled {
			t.Errorf("%s: err = %v, want pooled=%v", c.url, err, c.pooled)
			continue
		}
		if err != nil {
			if !strings.Contains(err.Error(), "DATABASE_URL_UNPOOLED") {
				t.Errorf("message lacks the fix: %v", err)
			}
			if strings.Contains(err.Error(), ":p@") {
				t.Errorf("message leaks credentials: %v", err)
			}
		}
	}
}

// MigrateURL refuses a pooled host before trying to connect.
func TestMigrateURLRejectsPooler(t *testing.T) {
	err := MigrateURL(context.Background(), "postgres://u:p@ep-x-pooler.invalid/db?connect_timeout=1")
	if err == nil || !strings.Contains(err.Error(), "DATABASE_URL_UNPOOLED") {
		t.Fatalf("err = %v", err)
	}
}
