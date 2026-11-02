package fsutil

import (
	"fmt"
	"os"
	"path/filepath"
	"testing"
)

func TestConcurrentCreateNewHasOneWinner(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "shared.toml")
	type result struct {
		content string
		created bool
		err     error
	}
	const writers = 12
	start := make(chan struct{})
	results := make(chan result, writers)
	for i := 0; i < writers; i++ {
		go func(i int) {
			content := fmt.Sprintf("writer-%d\n", i)
			<-start
			created, err := Write(path, []byte(content), CreateNewWithParents)
			results <- result{content, created, err}
		}(i)
	}
	close(start)
	winners, winner := 0, ""
	for i := 0; i < writers; i++ {
		result := <-results
		if result.err != nil {
			t.Errorf("create-new failed: %v", result.err)
		}
		if result.created {
			winners++
			winner = result.content
		}
	}
	if content, err := os.ReadFile(path); err != nil || winners != 1 || string(content) != winner {
		t.Fatalf("winners=%d winner=%q content=%q error=%v", winners, winner, content, err)
	}
	if entries, err := os.ReadDir(dir); err != nil || len(entries) != 1 || entries[0].Name() != "shared.toml" {
		t.Fatalf("temporary files left behind: %v %v", entries, err)
	}
}

func TestWriteModes(t *testing.T) {
	for _, tc := range []struct {
		name      string
		mode      Mode
		parents   bool
		noReplace bool
	}{
		{"replace", Replace, false, false},
		{"create new", CreateNew, false, true},
		{"replace with parents", ReplaceWithParents, true, false},
		{"create new with parents", CreateNewWithParents, true, true},
	} {
		t.Run(tc.name, func(t *testing.T) {
			dir := t.TempDir()
			path := filepath.Join(dir, "nested", "task.md")
			if !tc.parents {
				if created, err := Write(path, []byte("first"), tc.mode); err == nil || created {
					t.Fatalf("missing parent accepted: %t %v", created, err)
				}
				if err := os.Mkdir(filepath.Dir(path), 0o755); err != nil {
					t.Fatal(err)
				}
			}
			if created, err := Write(path, []byte("first"), tc.mode); err != nil || !created {
				t.Fatalf("first write: %t %v", created, err)
			}
			created, err := Write(path, []byte("second"), tc.mode)
			if err != nil || created == tc.noReplace {
				t.Fatalf("second write: %t %v", created, err)
			}
			want := "second"
			if tc.noReplace {
				want = "first"
			}
			if data, err := os.ReadFile(path); err != nil || string(data) != want {
				t.Fatalf("content: %q %v", data, err)
			}
		})
	}
	path := filepath.Join(t.TempDir(), "invalid")
	for _, mode := range []Mode{0, CreateNewWithParents + 1} {
		if created, err := Write(path, []byte("data"), mode); err == nil || created {
			t.Fatalf("invalid mode %d accepted: %t %v", mode, created, err)
		}
		if _, err := os.Stat(path); !os.IsNotExist(err) {
			t.Fatalf("invalid mode %d wrote file: %v", mode, err)
		}
	}
}

func TestAtomicWriteAndCreateNewCollision(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "nested", "task.md")
	if created, err := Write(path, []byte("first\n"), CreateNewWithParents); err != nil || !created {
		t.Fatalf("initial write: %t %v", created, err)
	}
	if created, err := Write(path, []byte("collision\n"), CreateNewWithParents); err != nil || created {
		t.Fatalf("collision: %t %v", created, err)
	}
	if data, err := os.ReadFile(path); err != nil || string(data) != "first\n" {
		t.Fatalf("collision overwrote file: %q %v", data, err)
	}
	if created, err := Write(path, []byte("replacement\n"), Replace); err != nil || !created {
		t.Fatalf("replace: %t %v", created, err)
	}
	if data, err := os.ReadFile(path); err != nil || string(data) != "replacement\n" {
		t.Fatalf("replace: %q %v", data, err)
	}
	entries, err := os.ReadDir(filepath.Dir(path))
	if err != nil || len(entries) != 1 || entries[0].Name() != "task.md" {
		t.Fatalf("temporary files left behind: %v %v", entries, err)
	}
}
