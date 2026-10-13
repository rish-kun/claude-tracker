// Command migrate applies database migrations.
package main

import (
	"context"
	"log"
	"os"

	"cctrack/server/internal/db"
)

func main() {
	url := os.Getenv("DATABASE_URL_UNPOOLED")
	if url == "" {
		url = os.Getenv("DATABASE_URL")
	}
	if url == "" {
		log.Fatal("set DATABASE_URL (or DATABASE_URL_UNPOOLED)")
	}
	if err := db.MigrateURL(context.Background(), url); err != nil {
		log.Fatalf("migrate: %v", err)
	}
	log.Print("migrations applied")
}
