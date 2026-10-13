// Package store holds all SQL access.
package store

import (
	"errors"

	"github.com/jackc/pgx/v5/pgxpool"
)

var (
	ErrNotFound  = errors.New("not found")
	ErrForbidden = errors.New("forbidden")
)

type Store struct {
	pool *pgxpool.Pool
}

func New(pool *pgxpool.Pool) *Store { return &Store{pool: pool} }
