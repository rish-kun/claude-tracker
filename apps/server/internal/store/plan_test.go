package store

import "testing"

func TestGuessPlan(t *testing.T) {
	cases := map[[2]string]string{
		{"claude_pro", "default_claude_ai"}:      "pro",
		{"claude_max", "default_claude_max_20x"}: "max_20x",
		{"claude_max", "default_claude_max_5x"}:  "max_5x",
		{"claude_max", ""}:                       "unknown",
		{"claude_team", "default_claude_team"}:   "team",
		{"claude_enterprise", "anything"}:        "enterprise",
		{"", ""}:                                 "unknown",
	}
	for in, want := range cases {
		if got := GuessPlan(in[0], in[1]); got != want {
			t.Errorf("GuessPlan(%q,%q) = %q, want %q", in[0], in[1], got, want)
		}
	}
}
