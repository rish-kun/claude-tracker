package httpapi

import (
	"errors"
	"time"

	"cctrack/server/internal/store"
)

const maxRange = 400 * 24 * time.Hour

func parseRange(from, to time.Time, tz *string) (store.Filter, error) {
	zone := "UTC"
	if tz != nil && *tz != "" {
		zone = *tz
	}
	// Go resolves "Local" to the server's own zone, which Postgres does not know.
	if _, err := time.LoadLocation(zone); err != nil || zone == "Local" {
		return store.Filter{}, errors.New("unknown tz: " + zone)
	}
	if !to.After(from) {
		return store.Filter{}, errors.New("to must be after from")
	}
	if to.Sub(from) > maxRange {
		return store.Filter{}, errors.New("range must be at most 400 days")
	}
	return store.Filter{From: from, To: to, TZ: zone}, nil
}

func pageParams(limit, offset *int) (int, int) {
	l, o := 50, 0
	if limit != nil && *limit > 0 {
		l = min(*limit, 200)
	}
	if offset != nil && *offset > 0 {
		o = *offset
	}
	return l, o
}
