package vault_test

import (
	"os"
	"path/filepath"
	"reflect"
	"testing"

	"github.com/vieitesss/tt/internal/vault"
)

func TestCrossStoreMoveCopiesSubtreeAndRejectsCollisions(t *testing.T) {
	sourceDir, targetDir := t.TempDir(), t.TempDir()
	writeTasks(t, sourceDir, map[string]string{
		"a.md": "---\nid: a\ntitle: A\nstate: open\nrank: 0\n---\n[[outside]]\n",
		"b.md": "---\nid: b\ntitle: B\nstate: done\nparent: a\n---\nbody",
	})
	writeTasks(t, targetDir, map[string]string{"b.md": "malformed but occupied"})
	source, _ := vault.Open(sourceDir)
	target, _ := vault.Open(targetDir)
	if _, err := source.MoveTo(target, []string{"a"}, nil); err == nil {
		t.Fatal("existing target file must block move")
	}
	if _, err := os.Stat(filepath.Join(sourceDir, "a.md")); err != nil {
		t.Fatal("collision removed source", err)
	}
	if err := os.Remove(filepath.Join(targetDir, "b.md")); err != nil {
		t.Fatal(err)
	}
	moved, err := source.MoveTo(target, []string{"a"}, nil)
	if err != nil || len(moved) != 2 || moved[0].ID != "a" || moved[0].Rank != nil || moved[1].Parent == nil || *moved[1].Parent != "a" {
		t.Fatalf("move: %+v %v", moved, err)
	}
	if _, err := os.Stat(filepath.Join(sourceDir, "a.md")); !os.IsNotExist(err) {
		t.Fatal("source still exists")
	}
	if got := target.Links("a"); !reflect.DeepEqual(got, []string{"outside"}) {
		t.Fatalf("links changed: %v", got)
	}
}

func TestMoveWithinStoreRejectsCyclesAndPreservesSubtree(t *testing.T) {
	dir := t.TempDir()
	store, err := vault.Open(dir)
	if err != nil {
		t.Fatal(err)
	}
	root, _ := store.Add(vault.AddInput{Title: "Root"})
	child, _ := store.Add(vault.AddInput{Title: "Child", Parent: &root.ID})
	other, _ := store.Add(vault.AddInput{Title: "Other"})
	if _, err := store.SetParent(root.ID, &child.ID); err == nil {
		t.Fatal("expected cycle rejection")
	}
	if _, err := store.SetParent(root.ID, &other.ID); err != nil {
		t.Fatal(err)
	}
	if got := store.Children(other.ID); !reflect.DeepEqual(got, []string{root.ID}) {
		t.Fatalf("destination children: %v", got)
	}
	if got := store.Children(root.ID); !reflect.DeepEqual(got, []string{child.ID}) {
		t.Fatalf("subtree children: %v", got)
	}
}

func TestRenameSyncsOnlyMirrorsAndIndexesLinks(t *testing.T) {
	dir := t.TempDir()
	fixtures := map[string]string{
		"target.md": "---\nid: target\ntitle: Old\nstate: open\n---\n",
		"source.md": "---\nid: source\ntitle: Source\nstate: open\n---\n[[target.md|Old]] [[target|context]] `[[target|Old]]`\n```\n[[target|Old]]\n```\n[[missing]]",
	}
	for name, data := range fixtures {
		if err := os.WriteFile(filepath.Join(dir, name), []byte(data), 0o600); err != nil {
			t.Fatal(err)
		}
	}
	store, err := vault.Open(dir)
	if err != nil {
		t.Fatal(err)
	}
	if !reflect.DeepEqual(store.Links("source"), []string{"target", "missing"}) || !reflect.DeepEqual(store.Backlinks("target"), []string{"source"}) {
		t.Fatalf("links=%v backlinks=%v", store.Links("source"), store.Backlinks("target"))
	}
	_, count, err := store.SetTitle("target", "New")
	if err != nil || count != 1 {
		t.Fatalf("rename: count %d, %v", count, err)
	}
	source, _ := store.Get("source")
	want := "[[target.md|New]] [[target|context]] `[[target|Old]]`\n```\n[[target|Old]]\n```\n[[missing]]"
	if source.Body != want {
		t.Fatalf("body = %q", source.Body)
	}
}

func TestAddPersistsTreeAndAppendsSiblingRanks(t *testing.T) {
	dir := t.TempDir()
	store, err := vault.Open(dir)
	if err != nil {
		t.Fatal(err)
	}
	parent, err := store.Add(vault.AddInput{Title: "Parent"})
	if err != nil {
		t.Fatal(err)
	}
	child, err := store.Add(vault.AddInput{Title: "Child", Parent: &parent.ID, Tags: []string{"#work/admin"}, Body: "hello"})
	if err != nil {
		t.Fatal(err)
	}
	if child.Parent == nil || *child.Parent != parent.ID || child.Rank == nil || *child.Rank != 0 {
		t.Fatalf("unexpected child: %+v", child)
	}
	second, err := store.Add(vault.AddInput{Title: "Another", Parent: &parent.ID})
	if err != nil {
		t.Fatal(err)
	}
	if second.Rank == nil || *second.Rank != 1 || len(store.Children(parent.ID)) != 2 || store.Children(parent.ID)[0] != child.ID {
		t.Fatalf("wrong append order: %+v, %v", second, store.Children(parent.ID))
	}
	reopened, err := vault.Open(dir)
	if err != nil || len(reopened.Roots()) != 1 || len(reopened.Children(parent.ID)) != 2 {
		t.Fatalf("reopen: %v, %v", reopened, err)
	}
	if _, err := os.Stat(filepath.Join(dir, child.ID+".md")); err != nil {
		t.Fatal(err)
	}
}
