package fsdfeed

import "testing"

func TestFeedNotifiesAfterAuthoritySnapshotChanges(t *testing.T) {
	f := NewFeed("http://unused.invalid")
	called := 0
	f.SetChangeHook(func() {
		called++
		if f.Degraded() {
			t.Fatal("callback ran before new snapshot became available")
		}
	})
	if err := f.applyEvent("snapshot", []byte(`{"pilots":[],"controllers":[],"atis":[]}`)); err != nil {
		t.Fatal(err)
	}
	if called != 1 {
		t.Fatalf("change notifications = %d, want 1", called)
	}
}
