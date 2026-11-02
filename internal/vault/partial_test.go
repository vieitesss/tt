package vault

import (
	"errors"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/vieitesss/tt/internal/fsutil"
)

func fixtureFile(t *testing.T, dir, id, fields, body string) {
	t.Helper()
	text := "---\nid: " + id + "\ntitle: " + strings.ToUpper(id) + "\nstate: open\n" + fields + "---\n" + body
	if err := os.WriteFile(filepath.Join(dir, id+".md"), []byte(text), 0o644); err != nil {
		t.Fatal(err)
	}
}

func TestApplyRevisionMatchesBytesUsedForPlanning(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "a.md")
	original := "---\nid: a\ntitle: A\nstate: open\n---\nOriginal body\n"
	external := strings.Replace(original, "Original body", "External body", 1)
	if err := os.WriteFile(path, []byte(external), 0o644); err != nil {
		t.Fatal(err)
	}
	v, err := Open(dir)
	if err != nil {
		t.Fatal(err)
	}
	revision, err := v.Revision()
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(path, []byte(original), 0o644); err != nil {
		t.Fatal(err)
	}
	v.readFile = func(name string) ([]byte, error) {
		data, err := os.ReadFile(name)
		if err == nil && name == path {
			if writeErr := os.WriteFile(path, []byte(external), 0o644); writeErr != nil {
				return nil, writeErr
			}
		}
		return data, err
	}
	a := "a"
	_, err = v.Apply(ApplyInput{Revision: revision, Tasks: []DesiredTask{{ID: &a, Title: "Updated"}}}, false)
	if err == nil || !strings.Contains(err.Error(), "task list changed on disk; reload") {
		t.Fatalf("apply planned from a different snapshot: %v", err)
	}
	data, err := os.ReadFile(path)
	if err != nil || string(data) != external {
		t.Fatalf("external edit lost: %q %v", data, err)
	}
}

func TestApplyRejectsTaskAddedDuringPlanning(t *testing.T) {
	dir := t.TempDir()
	fixtureFile(t, dir, "a", "", "")
	v, err := Open(dir)
	if err != nil {
		t.Fatal(err)
	}
	revision, err := v.Revision()
	if err != nil {
		t.Fatal(err)
	}
	v.readFile = func(name string) ([]byte, error) {
		data, err := os.ReadFile(name)
		if err == nil && name == filepath.Join(dir, "a.md") {
			fixtureFile(t, dir, "b", "", "New body")
		}
		return data, err
	}
	a := "a"
	_, err = v.Apply(ApplyInput{Revision: revision, Tasks: []DesiredTask{{ID: &a, Title: "Updated"}}}, false)
	if err == nil || !strings.Contains(err.Error(), "task list changed on disk; reload") {
		t.Fatalf("apply accepted a newly added task: %v", err)
	}
	if task, ok := v.Get("a"); !ok || task.Title != "A" {
		t.Fatalf("stale apply changed existing task: %+v", task)
	}
	if data, err := os.ReadFile(filepath.Join(dir, "b.md")); err != nil || !strings.Contains(string(data), "New body") {
		t.Fatalf("new task lost: %q %v", data, err)
	}
}

func TestApplyPartialFailureReportsOnlyCommittedChanges(t *testing.T) {
	dir := t.TempDir()
	fixtureFile(t, dir, "a", "", "")
	fixtureFile(t, dir, "b", "", "")
	v, err := Open(dir)
	if err != nil {
		t.Fatal(err)
	}
	original, err := v.Revision()
	if err != nil {
		t.Fatal(err)
	}
	v.writeFile = func(path string, data []byte, mode fsutil.Mode) (bool, error) {
		if filepath.Base(path) == "b.md" {
			return false, os.ErrPermission
		}
		return fsutil.Write(path, data, mode)
	}
	a, b, done := "a", "b", "done"
	_, err = v.Apply(ApplyInput{Revision: original, Tasks: []DesiredTask{{ID: &a, Title: "A", State: &done}, {ID: &b, Title: "B", State: &done}}}, false)
	var partial *PartialApplyError
	if !errors.As(err, &partial) || !errors.Is(err, os.ErrPermission) {
		t.Fatalf("expected partial permission failure: %v", err)
	}
	if len(partial.Applied.Updates) != 1 || partial.Applied.Updates[0].ID != "a" || len(partial.Applied.Updates[0].Fields) != 1 || partial.Applied.Updates[0].Fields[0] != "state" || len(partial.Applied.Adds) != 0 {
		t.Fatalf("incorrect partial progress: %+v", partial.Applied)
	}
	if task, _ := v.Get("a"); task.State != "done" {
		t.Fatalf("committed task: %+v", task)
	}
	if task, _ := v.Get("b"); task.State != "open" {
		t.Fatalf("failed task: %+v", task)
	}
	current, err := v.Revision()
	if err != nil || current == original || partial.Applied.Revision != current {
		t.Fatalf("partial revision %s current %s: %v", partial.Applied.Revision, current, err)
	}
}

func TestRenameAndApplyPartialMirrorFailure(t *testing.T) {
	for _, apply := range []bool{false, true} {
		t.Run(map[bool]string{false: "rename", true: "apply"}[apply], func(t *testing.T) {
			dir := t.TempDir()
			fixtureFile(t, dir, "a", "", "")
			fixtureFile(t, dir, "link", "", "[[a|A]]")
			v, err := Open(dir)
			if err != nil {
				t.Fatal(err)
			}
			v.writeFile = func(path string, data []byte, mode fsutil.Mode) (bool, error) {
				if filepath.Base(path) == "link.md" {
					return false, os.ErrPermission
				}
				return fsutil.Write(path, data, mode)
			}
			if apply {
				revision, _ := v.Revision()
				a, link := "a", "link"
				_, err = v.Apply(ApplyInput{Revision: revision, Tasks: []DesiredTask{{ID: &a, Title: "New"}, {ID: &link, Title: "LINK"}}}, false)
				var partial *PartialApplyError
				if !errors.As(err, &partial) || len(partial.Applied.Updates) != 1 || partial.Applied.Updates[0].ID != "a" {
					t.Fatalf("apply progress: %+v %v", partial, err)
				}
			} else {
				_, count, err := v.SetTitle("a", "New")
				if err == nil || count != 0 || !strings.Contains(err.Error(), "title_committed=true") {
					t.Fatalf("rename partial: %d %v", count, err)
				}
			}
			if task, _ := v.Get("a"); task.Title != "New" {
				t.Fatalf("title not committed: %+v", task)
			}
			if task, _ := v.Get("link"); task.Body != "[[a|A]]" {
				t.Fatalf("failed mirror was modified: %+v", task)
			}
		})
	}
}

func TestReloadPartialFailureKeepsFreshCacheAndPriorMirrorWrites(t *testing.T) {
	dir := t.TempDir()
	fixtures := map[string]string{
		"a.md":  "---\nid: a\ntitle: Old\nstate: open\n---\n",
		"m1.md": "---\nid: m1\ntitle: Mirror 1\nstate: open\n---\n[[a|Old]]",
		"m2.md": "---\nid: m2\ntitle: Mirror 2\nstate: open\n---\n[[a|Old]]",
	}
	for name, text := range fixtures {
		if err := os.WriteFile(filepath.Join(dir, name), []byte(text), 0o644); err != nil {
			t.Fatal(err)
		}
	}
	v, err := Open(dir)
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(dir, "a.md"), []byte(strings.Replace(fixtures["a.md"], "Old", "New", 1)), 0o644); err != nil {
		t.Fatal(err)
	}
	failPath := filepath.Join(dir, "m2.md")
	v.writeFile = func(path string, data []byte, mode fsutil.Mode) (bool, error) {
		if path == failPath {
			return false, os.ErrPermission
		}
		return fsutil.Write(path, data, mode)
	}
	err = v.Reload()
	if !errors.Is(err, os.ErrPermission) || !strings.Contains(err.Error(), "1 mirror file(s) updated") {
		t.Fatalf("expected one committed mirror: %v", err)
	}
	if task, _ := v.Get("a"); task.Title != "New" {
		t.Fatalf("external edit not committed in cache: %+v", task)
	}
	if task, _ := v.Get("m1"); task.Body != "[[a|New]]" {
		t.Fatalf("first mirror not committed in cache: %+v", task)
	}
	if task, _ := v.Get("m2"); task.Body != "[[a|Old]]" {
		t.Fatalf("failed mirror was changed: %+v", task)
	}
	reopened, err := Open(dir)
	if err != nil {
		t.Fatal(err)
	}
	for _, id := range []string{"a", "m1", "m2"} {
		inMemory, _ := v.Get(id)
		onDisk, _ := reopened.Get(id)
		if inMemory.Title != onDisk.Title || inMemory.Body != onDisk.Body {
			t.Errorf("cache differs from disk for %s: %+v %+v", id, inMemory, onDisk)
		}
	}
}

func TestApplyPartialDeleteFailureKeepsRemainingTasks(t *testing.T) {
	dir := t.TempDir()
	fixtureFile(t, dir, "a", "", "")
	fixtureFile(t, dir, "b", "", "")
	v, err := Open(dir)
	if err != nil {
		t.Fatal(err)
	}
	revision, _ := v.Revision()
	v.removeFile = func(path string) error {
		if filepath.Base(path) == "b.md" {
			return os.ErrPermission
		}
		return os.Remove(path)
	}
	_, err = v.Apply(ApplyInput{Revision: revision, Tasks: []DesiredTask{}}, false)
	var partial *PartialApplyError
	if !errors.As(err, &partial) || len(partial.Applied.Deletes) != 1 || partial.Applied.Deletes[0].ID != "a" {
		t.Fatalf("delete progress: %+v %v", partial, err)
	}
	if _, err := os.Stat(filepath.Join(dir, "a.md")); !errors.Is(err, os.ErrNotExist) {
		t.Fatalf("committed delete missing: %v", err)
	}
	if _, err := os.Stat(filepath.Join(dir, "b.md")); err != nil {
		t.Fatalf("failed delete lost task: %v", err)
	}
}

func TestCrossStoreMoveWriteFailureAndConcurrentSourceEdit(t *testing.T) {
	for _, tc := range []struct {
		name       string
		modify     bool
		wantCounts string
	}{
		{"target write failure", false, "target_written=1, source_removed=0"},
		{"source changed after copy", true, "target_written=2, source_removed=0"},
	} {
		t.Run(tc.name, func(t *testing.T) {
			sourceDir, targetDir := t.TempDir(), t.TempDir()
			fixtureFile(t, sourceDir, "a", "", "old body")
			fixtureFile(t, sourceDir, "b", "parent: a\n", "child body")
			source, err := Open(sourceDir)
			if err != nil {
				t.Fatal(err)
			}
			target, err := Open(targetDir)
			if err != nil {
				t.Fatal(err)
			}
			target.writeFile = func(path string, data []byte, mode fsutil.Mode) (bool, error) {
				if filepath.Base(path) == "b.md" && !tc.modify {
					return false, os.ErrPermission
				}
				created, err := fsutil.Write(path, data, mode)
				if err == nil && filepath.Base(path) == "b.md" && tc.modify {
					path := filepath.Join(sourceDir, "a.md")
					data, readErr := os.ReadFile(path)
					if readErr != nil {
						return created, readErr
					}
					err = os.WriteFile(path, []byte(strings.Replace(string(data), "old body", "external change", 1)), 0o644)
				}
				return created, err
			}
			_, err = source.MoveTo(target, []string{"a"}, nil)
			if err == nil || !strings.Contains(err.Error(), tc.wantCounts) {
				t.Fatalf("expected safe partial move: %v", err)
			}
			for _, id := range []string{"a", "b"} {
				if _, err := os.Stat(filepath.Join(sourceDir, id+".md")); err != nil {
					t.Errorf("source %s lost: %v", id, err)
				}
			}
			if _, err := os.Stat(filepath.Join(targetDir, "a.md")); err != nil {
				t.Fatalf("completed target copy lost: %v", err)
			}
			if tc.modify {
				if task, ok := source.Get("a"); !ok || task.Body != "external change" {
					t.Fatalf("external edit not retained in source cache: %+v", task)
				}
			}
		})
	}
}

func TestCrossStoreMoveRejectsRacingTargetCollision(t *testing.T) {
	sourceDir, targetDir := t.TempDir(), t.TempDir()
	fixtureFile(t, sourceDir, "a", "", "source body")
	source, err := Open(sourceDir)
	if err != nil {
		t.Fatal(err)
	}
	target, err := Open(targetDir)
	if err != nil {
		t.Fatal(err)
	}
	target.writeFile = func(path string, data []byte, mode fsutil.Mode) (bool, error) {
		if err := os.WriteFile(path, []byte("racing target"), 0o644); err != nil {
			return false, err
		}
		return fsutil.Write(path, data, mode)
	}
	_, err = source.MoveTo(target, []string{"a"}, nil)
	if err == nil || !strings.Contains(err.Error(), "target_written=0") {
		t.Fatalf("racing collision accepted: %v", err)
	}
	if data, err := os.ReadFile(filepath.Join(targetDir, "a.md")); err != nil || string(data) != "racing target" {
		t.Fatalf("racing target overwritten: %q %v", data, err)
	}
	if data, err := os.ReadFile(filepath.Join(sourceDir, "a.md")); err != nil || !strings.Contains(string(data), "source body") {
		t.Fatalf("source lost: %q %v", data, err)
	}
}

func TestCrossStoreMovePartialFailureKeepsSafeCopies(t *testing.T) {
	sourceDir, targetDir := t.TempDir(), t.TempDir()
	fixtureFile(t, sourceDir, "a", "", "")
	fixtureFile(t, sourceDir, "b", "parent: a\n", "body\n")
	source, err := Open(sourceDir)
	if err != nil {
		t.Fatal(err)
	}
	target, err := Open(targetDir)
	if err != nil {
		t.Fatal(err)
	}
	source.removeFile = func(string) error { return os.ErrPermission }
	_, err = source.MoveTo(target, []string{"a"}, nil)
	if err == nil || !strings.Contains(err.Error(), "target_written=2, source_removed=0") || !errors.Is(err, os.ErrPermission) {
		t.Fatalf("partial move: %v", err)
	}
	for _, dir := range []string{sourceDir, targetDir} {
		for _, id := range []string{"a", "b"} {
			if _, err := os.Stat(filepath.Join(dir, id+".md")); err != nil {
				t.Fatalf("unsafe partial move lost %s/%s: %v", dir, id, err)
			}
		}
	}
}
