package vault_test

import (
	"os"
	"path/filepath"
	"reflect"
	"strings"
	"testing"

	"github.com/vieitesss/tt/internal/vault"
)

func TestRepositoryNotesAreSilentlyIgnored(t *testing.T) {
	notes, err := filepath.Glob(filepath.Join("..", "..", "*.md"))
	if err != nil || len(notes) == 0 {
		t.Fatalf("repository notes not found: %v", err)
	}
	dir := t.TempDir()
	for _, note := range notes {
		data, err := os.ReadFile(note)
		if err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(filepath.Join(dir, filepath.Base(note)), data, 0o644); err != nil {
			t.Fatal(err)
		}
	}
	v, err := vault.Open(dir)
	if err != nil {
		t.Fatal(err)
	}
	if len(v.Issues()) != 0 || len(v.Tasks()) != 0 {
		t.Fatalf("notes are not foreign: %+v", v.Issues())
	}
}

func TestFilesWithTheSameDeclaredIDRemainDistinct(t *testing.T) {
	dir := t.TempDir()
	writeTasks(t, dir, map[string]string{
		"a.md": "---\nid: shared\ntitle: First\nstate: open\n---\n",
		"b.md": "---\nid: shared\ntitle: Second\nstate: open\n---\n",
	})
	v, err := vault.Open(dir)
	if err != nil {
		t.Fatal(err)
	}
	if len(v.Tasks()) != 2 || len(v.Issues()) != 2 {
		t.Fatalf("declared ids collapsed: %+v", v.Issues())
	}
	if _, _, err := v.SetTitle("a", "Changed"); err != nil {
		t.Fatal(err)
	}
	for _, id := range []string{"a", "b"} {
		if task, ok := v.Get(id); !ok || task.DeclaredID != "shared" {
			t.Errorf("lost declared id for %s: %+v", id, task)
		}
		if data, err := os.ReadFile(filepath.Join(dir, id+".md")); err != nil || !strings.Contains(string(data), "id: shared\n") {
			t.Errorf("changed declared id in %s: %q %v", id, data, err)
		}
	}
}

func TestMoveOverlappingSubtreesAndPreservesSurvivors(t *testing.T) {
	dir := t.TempDir()
	fixtures := map[string]string{
		"root.md":  "---\nid: root\ntitle: Root\nstate: open\n---\n",
		"a.md":     "---\nid: a\ntitle: A\nstate: open\nparent: root\n---\n",
		"b.md":     "---\nid: b\ntitle: B\nstate: open\nparent: a\n---\n",
		"c.md":     "---\nid: c\ntitle: C\nstate: open\nparent: b\n---\n",
		"keep.md":  "---\nid: keep\ntitle: Keep\nstate: open\ncustom: keep-me\n---\nsee [[a]]",
		"other.md": "---\nid: other\ntitle: Other\nstate: open\n---\n",
	}
	writeTasks(t, dir, fixtures)
	v, err := vault.Open(dir)
	if err != nil {
		t.Fatal(err)
	}
	target, err := vault.Open(t.TempDir())
	if err != nil {
		t.Fatal(err)
	}
	moved, err := v.MoveTo(target, []string{"a", "b"}, nil)
	if err != nil || len(moved) != 3 || moved[0].ID != "a" || moved[1].ID != "b" || moved[2].ID != "c" {
		t.Fatalf("overlapping move: %+v %v", moved, err)
	}
	if moved[0].Parent != nil || moved[1].Parent == nil || *moved[1].Parent != "a" || moved[2].Parent == nil || *moved[2].Parent != "b" {
		t.Fatalf("moved subtree structure: %+v", moved)
	}
	for _, id := range []string{"a", "b", "c"} {
		if _, ok := v.Get(id); ok {
			t.Errorf("deleted task %s remains in index", id)
		}
		if _, err := os.Stat(filepath.Join(dir, id+".md")); !os.IsNotExist(err) {
			t.Errorf("deleted task %s remains on disk: %v", id, err)
		}
	}
	for _, id := range []string{"root", "keep", "other"} {
		if data, err := os.ReadFile(filepath.Join(dir, id+".md")); err != nil || string(data) != fixtures[id+".md"] {
			t.Errorf("survivor %s changed: %q %v", id, data, err)
		}
	}
	if !reflect.DeepEqual(v.Links("keep"), []string{"a"}) {
		t.Fatalf("dangling link should remain: %v", v.Links("keep"))
	}
}

func TestSubtreeHandlesDanglingParentsAndCycles(t *testing.T) {
	for _, tc := range []struct {
		name  string
		files map[string]string
		want  []string
	}{
		{"dangling", map[string]string{
			"a.md": "---\nid: a\ntitle: A\nstate: open\nparent: missing\n---\n",
			"b.md": "---\nid: b\ntitle: B\nstate: open\nparent: a\n---\n",
		}, []string{"a", "b"}},
		{"cycle", map[string]string{
			"a.md": "---\nid: a\ntitle: A\nstate: open\nparent: b\n---\n",
			"b.md": "---\nid: b\ntitle: B\nstate: open\nparent: a\n---\n",
			"c.md": "---\nid: c\ntitle: C\nstate: open\nparent: a\n---\n",
		}, []string{"a", "b", "c"}},
	} {
		t.Run(tc.name, func(t *testing.T) {
			dir := t.TempDir()
			writeTasks(t, dir, tc.files)
			v, err := vault.Open(dir)
			if err != nil || len(v.Issues()) == 0 {
				t.Fatalf("expected issue: %v %v", v.Issues(), err)
			}
			var ids []string
			for _, task := range v.Subtree("a") {
				ids = append(ids, task.ID)
			}
			if !reflect.DeepEqual(ids, tc.want) {
				t.Fatalf("effective subtree: %v", ids)
			}
			if len(v.Subtree("missing")) != 0 {
				t.Fatal("missing subtree was not empty")
			}
		})
	}
}

func TestReloadObservesExternalRenamesAndPreservesIssues(t *testing.T) {
	dir := t.TempDir()
	fixtures := map[string]string{
		"a.md":          "---\nid: a\ntitle: Old\nstate: open\n---\n",
		"mirror.md":     "---\nid: mirror\ntitle: Mirror\nstate: open\n---\n[[a|Old]]",
		"foreign.md":    "---\ntitle: Note\n---\nbody",
		"broken.md":     "---\nid: broken\ntitle: [bad\n---\n",
		"declared.md":   "---\nid: mismatch\ntitle: Mismatch\nstate: open\n---\n",
		"unsafe ID.md":  "---\nid: good\ntitle: Invalid stem\nstate: open\n---\n",
		"ignored.txt":   "---\nid: ignored\ntitle: Ignored\nstate: open\n---\n",
		"plain-note.md": "ordinary markdown\n",
	}
	writeTasks(t, dir, fixtures)
	v, err := vault.Open(dir)
	if err != nil || len(v.Issues()) != 3 {
		t.Fatalf("issues: %+v %v", v.Issues(), err)
	}
	if err := os.WriteFile(filepath.Join(dir, "a.md"), []byte(strings.Replace(fixtures["a.md"], "Old", "New", 1)), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := v.Reload(); err != nil {
		t.Fatal(err)
	}
	if task, ok := v.Get("a"); !ok || task.Title != "New" {
		t.Fatalf("external edit not observed: %+v", task)
	}
	if mirror, _ := v.Get("mirror"); mirror.Body != "[[a|New]]" {
		t.Fatalf("external rename did not sync: %q", mirror.Body)
	}
	for _, name := range []string{"broken.md", "foreign.md", "unsafe ID.md", "plain-note.md"} {
		if data, err := os.ReadFile(filepath.Join(dir, name)); err != nil || string(data) != fixtures[name] {
			t.Errorf("issue/foreign file %s was changed: %q %v", name, data, err)
		}
	}
}
