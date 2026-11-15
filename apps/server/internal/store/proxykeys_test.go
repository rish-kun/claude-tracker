package store_test

import (
	"bytes"
	"context"
	"errors"
	"testing"

	"cctrack/server/internal/auth"
	"cctrack/server/internal/store"
	"cctrack/server/internal/testdb"
)

// TestProxyKeys: request (pending, one active key per user per device),
// approve, revoke, list order, and the sync list that drops revoked keys and
// keys of unlinked owners.
func TestProxyKeys(t *testing.T) {
	st := store.New(testdb.New(t))
	ctx := context.Background()
	alice := newUser(t, st, "alice@example.com")
	bob := newUser(t, st, "bob@example.com")
	aliceTok, err := st.CreateToken(ctx, alice, auth.HashToken("cct_alice"), "laptop")
	if err != nil {
		t.Fatal(err)
	}
	// bob has no CLI token: their key is listed but not linked.

	plainA1, hashA1 := auth.NewProxyKey()
	if len(plainA1) != len("ccp_")+43 || plainA1[:4] != "ccp_" {
		t.Fatalf("key format %q", plainA1[:4])
	}
	a1, err := st.RequestProxyKey(ctx, alice, hashA1, "mac")
	if err != nil {
		t.Fatal(err)
	}
	if a1.User.Email != "alice@example.com" || a1.DeviceName != "mac" || !a1.Linked || a1.RevokedAt != nil ||
		a1.ApprovedAt != nil || a1.Status() != store.ProxyKeyPending {
		t.Fatalf("requested %+v", a1)
	}
	_, hashPC := auth.NewProxyKey()
	pc, err := st.RequestProxyKey(ctx, alice, hashPC, "pc")
	if err != nil {
		t.Fatal(err)
	}
	_, hashB := auth.NewProxyKey()
	b1, err := st.RequestProxyKey(ctx, bob, hashB, "desktop")
	if err != nil {
		t.Fatal(err)
	}
	if b1.Linked {
		t.Fatal("bob has no CLI token but key is linked")
	}
	if _, err := st.RequestProxyKey(ctx, 999999, auth.HashToken("x"), "mac"); !errors.Is(err, store.ErrNotFound) {
		t.Fatalf("unknown user: %v, want ErrNotFound", err)
	}

	// Approve: once; never a revoked or missing key.
	if err := st.ApproveProxyKey(ctx, a1.ID); err != nil {
		t.Fatal(err)
	}
	if err := st.ApproveProxyKey(ctx, a1.ID); !errors.Is(err, store.ErrNotFound) {
		t.Fatalf("second approve: %v", err)
	}
	if err := st.ApproveProxyKey(ctx, 999999); !errors.Is(err, store.ErrNotFound) {
		t.Fatalf("approve missing: %v", err)
	}

	// A new request for the same device revokes the old key, approved or not;
	// other devices and other users keep theirs.
	_, hashA2 := auth.NewProxyKey()
	a2, err := st.RequestProxyKey(ctx, alice, hashA2, "mac")
	if err != nil {
		t.Fatal(err)
	}
	if a2.Status() != store.ProxyKeyPending {
		t.Fatalf("replacement should start pending: %+v", a2)
	}
	if err := st.ApproveProxyKey(ctx, a1.ID); !errors.Is(err, store.ErrNotFound) {
		t.Fatalf("approve replaced key: %v", err)
	}
	if err := st.RevokeProxyKey(ctx, a1.ID); !errors.Is(err, store.ErrNotFound) {
		t.Fatalf("replaced key should already be revoked: %v", err)
	}
	if err := st.ApproveProxyKey(ctx, pc.ID); err != nil {
		t.Fatal(err)
	}

	// Pending first (oldest first), then approved, then revoked.
	keys, err := st.ListProxyKeys(ctx)
	if err != nil {
		t.Fatal(err)
	}
	var got []int64
	for _, k := range keys {
		got = append(got, k.ID)
	}
	want := []int64{b1.ID, a2.ID, pc.ID, a1.ID}
	if len(got) != len(want) {
		t.Fatalf("list = %v, want %v", got, want)
	}
	for i := range want {
		if got[i] != want[i] {
			t.Fatalf("list = %v, want %v", got, want)
		}
	}
	if keys[2].Status() != store.ProxyKeyApproved || keys[2].ApprovedAt == nil {
		t.Fatalf("approved key: %+v", keys[2])
	}
	if keys[3].Status() != store.ProxyKeyRevoked || keys[3].ApprovedAt == nil {
		t.Fatalf("revoked key keeps its approval time: %+v", keys[3])
	}

	// Sync: alice's two unrevoked keys with their status; bob is not linked.
	sync, err := st.SyncProxyKeys(ctx)
	if err != nil {
		t.Fatal(err)
	}
	if len(sync) != 2 || !bytes.Equal(sync[0].Hash, hashPC) || !sync[0].Approved || sync[0].DeviceName != "pc" ||
		!bytes.Equal(sync[1].Hash, hashA2) || sync[1].Approved || sync[1].Email != "alice@example.com" {
		t.Fatalf("sync = %+v", sync)
	}

	if err := st.RevokeProxyKey(ctx, pc.ID); err != nil {
		t.Fatal(err)
	}
	if sync, _ := st.SyncProxyKeys(ctx); len(sync) != 1 || !bytes.Equal(sync[0].Hash, hashA2) {
		t.Fatalf("after revoke: %+v", sync)
	}

	// Revoking alice's only CLI token unlinks them: nothing left to sync.
	if err := st.RevokeToken(ctx, aliceTok); err != nil {
		t.Fatal(err)
	}
	if sync, err := st.SyncProxyKeys(ctx); err != nil || len(sync) != 0 {
		t.Fatalf("after unlink: %+v %v", sync, err)
	}
	keys, _ = st.ListProxyKeys(ctx)
	for _, k := range keys {
		if k.Linked {
			t.Fatalf("still linked: %+v", k)
		}
	}
}
