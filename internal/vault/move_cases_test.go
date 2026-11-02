package vault_test

import (
	"os"
	"path/filepath"
	"testing"

	"github.com/vieitesss/tt/internal/vault"
)

func TestMoveReadsLatestSourceAndRejectsChangedSourceBeforeWrites(t *testing.T) {
	for _, tc := range []struct {
		name       string
		newContent string
		remove     bool
		wantError  bool
	}{
		{"latest edit", "---\nid: a\ntitle: Latest\nstate: open\n---\nnew body", false, false},
		{"malformed", "---\nid: a\ntitle: [bad\n---\n", false, true},
		{"missing", "", true, true},
	} {
		t.Run(tc.name, func(t *testing.T) {
			sourceDir, targetDir := t.TempDir(), t.TempDir()
			writeTasks(t, sourceDir, map[string]string{"a.md": "---\nid: a\ntitle: Original\nstate: open\nrank: 1\n---\nold body"})
			source, err := vault.Open(sourceDir)
			if err != nil {
				t.Fatal(err)
			}
			target, err := vault.Open(targetDir)
			if err != nil {
				t.Fatal(err)
			}
			path := filepath.Join(sourceDir, "a.md")
			if tc.remove {
				err = os.Remove(path)
			} else {
				err = os.WriteFile(path, []byte(tc.newContent), 0o644)
			}
			if err != nil {
				t.Fatal(err)
			}
			moved, err := source.MoveTo(target, []string{"a"}, nil)
			if tc.wantError {
				if err == nil {
					t.Fatal("expected source preflight error")
				}
				if _, statErr := os.Stat(filepath.Join(targetDir, "a.md")); !os.IsNotExist(statErr) {
					t.Fatalf("target was written before source preflight: %v", statErr)
				}
				return
			}
			if err != nil || len(moved) != 1 || moved[0].Title != "Latest" || moved[0].Body != "new body" || moved[0].Rank != nil {
				t.Fatalf("latest move: %+v %v", moved, err)
			}
			if _, err := os.Stat(path); !os.IsNotExist(err) {
				t.Fatalf("source not removed: %v", err)
			}
		})
	}
}
