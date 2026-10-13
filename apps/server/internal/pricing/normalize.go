package pricing

import (
	"regexp"
	"strings"
)

var (
	bracketSuffix = regexp.MustCompile(`\[[^\]]*\]$`)
	regionPrefix  = regexp.MustCompile(`^(us|eu|apac|global|jp|au)\.`)
	bedrockVer    = regexp.MustCompile(`-v\d+(:\d+)?$`)
	dateSuffix    = regexp.MustCompile(`-\d{8}$`)
)

// Normalize maps provider-specific model ids (dated, Bedrock, Vertex, [1m])
// to the canonical id used as the price-table key.
func Normalize(model string) string {
	m := strings.ToLower(strings.TrimSpace(model))
	m = bracketSuffix.ReplaceAllString(m, "")
	if i := strings.IndexByte(m, '@'); i >= 0 {
		m = m[:i]
	}
	m = regionPrefix.ReplaceAllString(m, "")
	m = strings.TrimPrefix(m, "anthropic.")
	m = bedrockVer.ReplaceAllString(m, "")
	m = dateSuffix.ReplaceAllString(m, "")
	return m
}
