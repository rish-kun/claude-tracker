package store

import "strings"

// APIKeyAccount is the account_uuid the CLI sends for API-key authentication.
const APIKeyAccount = "api_key"

// GuessPlan infers a plan from Claude Code's oauthAccount fields
// (organizationType, organizationRateLimitTier). Admins can override it.
func GuessPlan(orgType, tier string) string {
	t := strings.ToLower(orgType + " " + tier)
	switch {
	case strings.Contains(t, "enterprise"):
		return "enterprise"
	case strings.Contains(t, "team"):
		return "team"
	case strings.Contains(t, "20x"):
		return "max_20x"
	case strings.Contains(t, "5x"):
		return "max_5x"
	case strings.Contains(t, "max"):
		return "unknown"
	case strings.Contains(t, "pro"):
		return "pro"
	}
	return "unknown"
}
