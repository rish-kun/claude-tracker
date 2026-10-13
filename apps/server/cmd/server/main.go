// Command server runs the cctrack API.
package main

import (
	"context"
	"log"
	"net/http"
	"os"
	"time"
	_ "time/tzdata" // embed zoneinfo: slim runtimes (e.g. Vercel) may have none, breaking ?tz=

	"cctrack/server/internal/auth"
	"cctrack/server/internal/config"
	"cctrack/server/internal/db"
	"cctrack/server/internal/httpapi"
	"cctrack/server/internal/store"
)

func main() {
	cfg, err := config.Load(os.Getenv)
	if err != nil {
		log.Fatalf("config: %v", err)
	}
	ctx := context.Background()
	if cfg.AutoMigrate {
		if err := db.MigrateURL(ctx, cfg.MigrateURL()); err != nil {
			log.Fatalf("migrate: %v", err)
		}
	}
	pool, err := db.Open(ctx, cfg.DatabaseURL)
	if err != nil {
		log.Fatalf("db: %v", err)
	}
	defer pool.Close()

	srv := httpapi.New(cfg, store.New(pool), auth.NewGoogle(ctx, cfg.GoogleClientID, cfg.GoogleClientSecret))
	httpSrv := &http.Server{
		Addr:              ":" + cfg.Port,
		Handler:           srv.Handler(),
		ReadHeaderTimeout: 10 * time.Second,
	}
	log.Printf("cctrack server listening on :%s", cfg.Port)
	log.Fatal(httpSrv.ListenAndServe())
}
